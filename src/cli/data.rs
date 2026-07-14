use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Local;
use clap::Subcommand;
use comfy_table::Table;
use tokio::sync::Semaphore;

use crate::data::models::{infer_market, Stock};
use crate::data::source;
use crate::data::Store;

/// data update 的并发上限，礼貌爬取、避免把接口打挂。
const UPDATE_CONCURRENCY: usize = 4;

#[derive(Subcommand)]
pub enum DataCmd {
    /// 添加股票到跟踪列表
    Add { codes: Vec<String> },
    /// 从跟踪列表移除
    Remove { code: String },
    /// 增量更新日K数据（不带参数则更新全部）
    Update { codes: Vec<String> },
    /// 查看已跟踪的股票列表
    List,
    /// 查看某只股票的数据范围和条数
    Info { code: String },
}

pub async fn run(cmd: DataCmd) -> Result<()> {
    let mut store = Store::open_default()?;
    match cmd {
        DataCmd::Add { codes } => add(&mut store, codes).await,
        DataCmd::Remove { code } => remove(&store, &code),
        DataCmd::Update { codes } => update(&mut store, codes).await,
        DataCmd::List => list(&store),
        DataCmd::Info { code } => info(&store, &code),
    }
}

async fn add(store: &mut Store, codes: Vec<String>) -> Result<()> {
    if codes.is_empty() {
        return Err(anyhow!("请提供至少一个股票代码"));
    }
    for code in codes {
        let market = infer_market(&code).ok_or_else(|| anyhow!("无法识别的股票代码 {}", code))?;
        let (name, klines, src) = source::fetch_klines(&code, market, "0", "20500101").await?;
        // 腾讯/新浪日K不返回名称，回退用批量行情接口补名，避免名称落成代码
        let name = if name.is_empty() {
            resolve_name(&code, market).await
        } else {
            name
        };
        let stock = Stock {
            code: code.clone(),
            name,
            market,
            added_at: Local::now().format("%Y-%m-%d").to_string(),
        };
        store.add_stock(&stock)?;
        let n = store.upsert_klines(&klines)?;
        println!(
            "已添加 {} {}，写入 {} 条日K（来源 {}）",
            stock.code, stock.name, n, src
        );
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

async fn update(store: &mut Store, codes: Vec<String>) -> Result<()> {
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
        let beg = match store.latest_kline_date(&stock.code)? {
            Some(d) => d.replace('-', ""),
            None => "0".to_string(),
        };
        jobs.push((stock, beg));
    }

    // 有界并发拉取网络数据：Semaphore 限并发 + 抖动打散 + 指数退避重试。
    // 只并行网络 IO，SQLite 写入仍在主任务串行完成（rusqlite 连接非线程安全）。
    let sem = Arc::new(Semaphore::new(UPDATE_CONCURRENCY));
    let mut handles = Vec::with_capacity(jobs.len());
    for (stock, beg) in jobs {
        let sem = sem.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(source::jitter_ms(400))).await;
            let fetched = source::with_retry(3, || {
                source::fetch_klines(&stock.code, stock.market, &beg, "20500101")
            })
            .await;
            (stock, fetched)
        }));
    }

    for handle in handles {
        let (stock, fetched) = handle.await?;
        match fetched {
            Ok((_, klines, src)) => {
                let n = store.upsert_klines(&klines)?;
                println!(
                    "{} {} 更新 {} 条（来源 {}）",
                    stock.code, stock.name, n, src
                );
            }
            Err(e) => eprintln!("{} {} 更新失败：{}", stock.code, stock.name, e),
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
        let count = store.kline_count(&s.code)?;
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

fn info(store: &Store, code: &str) -> Result<()> {
    let stock = store
        .get_stock(code)?
        .ok_or_else(|| anyhow!("{} 不在跟踪列表中", code))?;
    let count = store.kline_count(code)?;
    println!("代码：{}", stock.code);
    println!("名称：{}", stock.name);
    println!("市场：{}", stock.market.as_str());
    println!("K线条数：{}", count);
    match store.kline_date_range(code)? {
        Some((a, b)) => println!("数据范围：{} ~ {}", a, b),
        None => println!("数据范围：（无数据）"),
    }
    Ok(())
}
