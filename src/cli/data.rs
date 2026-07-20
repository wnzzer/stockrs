use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use comfy_table::Table;
use tokio::sync::Semaphore;

use crate::data::models::{normalize_code, Market, Period, Stock};
use crate::data::source;
use crate::data::Store;

/// data update 的并发上限，礼貌爬取、避免把接口打挂。
const UPDATE_CONCURRENCY: usize = 4;

/// clap 值解析：CLI 周期字符串 -> Period。
fn parse_period(s: &str) -> Result<Period, String> {
    Period::parse(s).ok_or_else(|| format!("未知周期 {}（可用 d/1m/5m/15m/30m/60m）", s))
}

#[derive(Subcommand)]
pub enum DataCmd {
    /// 添加股票到跟踪列表
    Add {
        codes: Vec<String>,
        /// K线周期：d(默认)/1m/5m/15m/30m/60m
        #[arg(long, default_value = "d", value_parser = parse_period)]
        period: Period,
    },
    /// 从跟踪列表移除
    Remove { code: String },
    /// 增量更新K线数据（不带参数则更新全部）
    Update {
        codes: Vec<String>,
        /// K线周期：d(默认)/1m/5m/15m/30m/60m
        #[arg(long, default_value = "d", value_parser = parse_period)]
        period: Period,
    },
    /// 查看已跟踪的股票列表
    List,
    /// 查看某只股票的数据范围和条数
    Info {
        code: String,
        /// K线周期：d(默认)/1m/5m/15m/30m/60m
        #[arg(long, default_value = "d", value_parser = parse_period)]
        period: Period,
    },
}

pub async fn run(cmd: DataCmd) -> Result<()> {
    let mut store = Store::open_default()?;
    match cmd {
        DataCmd::Add { codes, period } => add(&mut store, codes, period).await,
        DataCmd::Remove { code } => remove(&store, &code),
        DataCmd::Update { codes, period } => update(&mut store, codes, period).await,
        DataCmd::List => list(&store),
        DataCmd::Info { code, period } => info(&store, &code, period),
    }
}

async fn add(store: &mut Store, codes: Vec<String>, period: Period) -> Result<()> {
    if codes.is_empty() {
        return Err(anyhow!("请提供至少一个股票代码"));
    }
    for input in codes {
        let (code, market) =
            normalize_code(&input).ok_or_else(|| anyhow!("无法识别的代码 {}", input))?;
        let (name, klines, src) =
            source::fetch_klines(&code, market, period, "0", "20500101").await?;
        // 腾讯/新浪日K不返回名称，回退用批量行情接口补名，避免名称落成代码
        let name = if name.is_empty() {
            resolve_name(&code, market).await
        } else {
            name
        };
        // 每手股数:港股逐股不同,拉 F10 TRADE_UNIT;A股恒 100。失败回退 100。
        let lot_size = if market == Market::HK {
            match crate::data::hk::fetch_lot_size(&code).await {
                Ok(Some(l)) if l > 0 => l,
                _ => {
                    eprintln!("  {} 每手股数获取失败，暂按 100 股", code);
                    100
                }
            }
        } else {
            100
        };
        let stock = Stock {
            code: code.clone(),
            name,
            market,
            added_at: crate::utils::date::today(),
            lot_size,
        };
        store.add_stock(&stock)?;
        let n = store.upsert_klines(&klines, period)?;
        let lot_note = if market == Market::HK {
            format!("，每手 {} 股", lot_size)
        } else {
            String::new()
        };
        println!(
            "已添加 {} {}，写入 {} 条{}（来源 {}）{}",
            stock.code,
            stock.name,
            n,
            period.label(),
            src,
            lot_note
        );
        // 基本面为日频估值,仅日线抓取;分钟线跳过。失败仅告警,不影响已写入的 K 线。
        if !period.is_intraday() {
            match crate::data::fundamental::fetch(&code, market, None).await {
                Ok(f) if !f.is_empty() => {
                    let c = store.upsert_fundamentals(&f).unwrap_or(0);
                    println!("  基本面 {} 条(PE/PB/PS/市值)", c);
                }
                Ok(_) => {}
                Err(e) => eprintln!("  {} 基本面获取失败：{}", code, e),
            }
        }
    }
    Ok(())
}

/// 用行情接口取股票/基金名称，失败则回退成代码本身。
async fn resolve_name(code: &str, market: crate::data::Market) -> String {
    match source::fetch_quotes(&[(code.to_string(), market)]).await {
        Ok((qs, _)) if qs.first().is_some_and(|(q, _)| !q.name.is_empty()) => {
            qs.into_iter().next().unwrap().0.name
        }
        _ => code.to_string(),
    }
}

fn remove(store: &Store, code: &str) -> Result<()> {
    if store.remove_stock(code)? {
        println!("已移除 {}", code);
    } else {
        println!("{} 不在跟踪列表中", code);
    }
    Ok(())
}

async fn update(store: &mut Store, codes: Vec<String>, period: Period) -> Result<()> {
    let targets: Vec<Stock> = if codes.is_empty() {
        store.list_stocks()?
    } else {
        codes
            .iter()
            .filter_map(|c| store.get_stock(c).transpose())
            .collect::<Result<Vec<_>>>()?
    };
    if targets.is_empty() {
        println!("没有需要更新的股票，请先 data add");
        return Ok(());
    }
    // 计算每只股票的增量起点（读库，串行且廉价）
    let mut jobs = Vec::with_capacity(targets.len());
    for stock in targets {
        // 分钟线 date 形如 "2025-01-15 09:35",增量起点只取日期部分转 YYYYMMDD。
        let beg = match store.latest_kline_date(&stock.code, period)? {
            Some(d) => d[..d.len().min(10)].replace('-', ""),
            None => "0".to_string(),
        };
        // 基本面增量起点(YYYY-MM-DD);None 则全量。仅日线更新时抓取。
        let fund_since = if period.is_intraday() {
            None
        } else {
            store.latest_fundamental_date(&stock.code)?
        };
        jobs.push((stock, beg, fund_since));
    }

    // 有界并发拉取网络数据：Semaphore 限并发 + 抖动打散 + 指数退避重试。
    // 只并行网络 IO，SQLite 写入仍在主任务串行完成（rusqlite 连接非线程安全）。
    let sem = Arc::new(Semaphore::new(UPDATE_CONCURRENCY));
    let mut handles = Vec::with_capacity(jobs.len());
    for (stock, beg, fund_since) in jobs {
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(source::jitter_ms(400))).await;
            let fetched = source::with_retry(3, || {
                source::fetch_klines(&stock.code, stock.market, period, &beg, "20500101")
            })
            .await;
            // 分钟线不涉及基本面。
            let funda = if period.is_intraday() {
                Ok(Vec::new())
            } else {
                source::with_retry(3, || {
                    crate::data::fundamental::fetch(
                        &stock.code,
                        stock.market,
                        fund_since.as_deref(),
                    )
                })
                .await
            };
            (stock, fetched, funda)
        }));
    }

    for handle in handles {
        let (stock, fetched, funda) = handle.await?;
        match fetched {
            Ok((_, klines, src)) => {
                let n = store.upsert_klines(&klines, period)?;
                println!(
                    "{} {} 更新 {} 条{}（来源 {}）",
                    stock.code,
                    stock.name,
                    n,
                    period.label(),
                    src
                );
            }
            Err(e) => {
                // 已有历史数据时,增量拉不到新数据(某些源对已是最新的区间返回空,
                // 叠加另一源临时网络问题)属常见且无害,不当作失败告警。
                if store.kline_count(&stock.code, period).unwrap_or(0) > 0 {
                    println!(
                        "{} {} 无新数据（已是最新或数据源暂不可用）",
                        stock.code, stock.name
                    );
                } else {
                    eprintln!("{} {} 更新失败：{}", stock.code, stock.name, e);
                }
            }
        }
        // 基本面失败不阻断 K 线结果。
        match funda {
            Ok(f) if !f.is_empty() => {
                let c = store.upsert_fundamentals(&f).unwrap_or(0);
                println!("  {} 基本面更新 {} 条", stock.code, c);
            }
            Ok(_) => {}
            Err(e) => eprintln!("  {} 基本面更新失败：{}", stock.code, e),
        }
    }
    Ok(())
}

fn list(store: &Store) -> Result<()> {
    let stocks = store.list_stocks()?;
    if stocks.is_empty() {
        println!("跟踪列表为空");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec!["代码", "名称", "市场", "K线数", "添加时间"]);
    for s in stocks {
        let count = store.kline_count(&s.code, Period::Day)?;
        table.add_row(vec![
            s.code.clone(),
            s.name,
            s.market.as_str().to_string(),
            count.to_string(),
            s.added_at,
        ]);
    }
    println!("{table}");
    Ok(())
}

fn info(store: &Store, code: &str, period: Period) -> Result<()> {
    let stock = store
        .get_stock(code)?
        .ok_or_else(|| anyhow!("{} 不在跟踪列表中", code))?;
    let count = store.kline_count(code, period)?;
    println!("代码：{}", stock.code);
    println!("名称：{}", stock.name);
    println!("市场：{}", stock.market.as_str());
    println!("周期：{}", period.label());
    println!("K线条数：{}", count);
    match store.kline_date_range(code, period)? {
        Some((a, b)) => println!("数据范围：{} ~ {}", a, b),
        None => println!("数据范围：（无数据）"),
    }
    Ok(())
}
