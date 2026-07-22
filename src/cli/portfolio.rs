use crate::utils::date::{days_since, today};
use anyhow::{anyhow, Result};
use clap::Subcommand;
use comfy_table::Table;
use std::collections::HashMap;

use crate::data::models::normalize_code;
use crate::data::{benchmark, source, Market, Period, Position, Quote, Store};
use crate::engine::position_stats;
use crate::utils::format::{money, sparkline};

/// 交易日数少于此值时,极值/回撤/曲线样本太短、意义不大,略去(见持仓分析)。
const MIN_DAYS_FOR_CURVE: usize = 6;

#[derive(Subcommand)]
pub enum PortfolioCmd {
    /// 添加持仓
    Add {
        code: String,
        #[arg(long)]
        price: f64,
        #[arg(long)]
        quantity: i64,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// 卖出(减仓/清仓),记录已实现盈亏
    Sell {
        code: String,
        #[arg(long)]
        price: f64,
        #[arg(long)]
        quantity: i64,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// 移除持仓(不记录卖出,仅纠正误录)
    Remove { code: String },
    /// 账户仪表盘:持仓 + 今日涨跌/今日盈亏 + 今日/累计/已实现/总资产汇总
    List,
    /// 历史交易记录
    History,
    /// 持仓收益分析(收益曲线、回撤、日均收益、基准对比);省略代码或 --all 分析全部持仓
    Stats {
        code: Option<String>,
        /// 分析全部持仓(等价于省略代码)
        #[arg(long)]
        all: bool,
        /// 覆盖默认基准(hs300/zz500/kc50/cyb/sh/sz...);缺省按标的市场自动选
        #[arg(long)]
        benchmark: Option<String>,
    },
    /// 设置/查看现金余额(手动维护,计入仪表盘总资产)
    Cash { amount: Option<f64> },
}

pub async fn run(cmd: PortfolioCmd) -> Result<()> {
    let mut store = Store::open_default()?;
    match cmd {
        PortfolioCmd::Add {
            code,
            price,
            quantity,
            date,
            note,
        } => {
            let date = date.unwrap_or_else(today);
            store.add_position(&code, price, quantity, &date, note.as_deref())?;
            println!("已添加持仓 {} {}股 @ {}", code, quantity, price);
            Ok(())
        }
        PortfolioCmd::Sell {
            code,
            price,
            quantity,
            date,
            note,
        } => {
            let date = date.unwrap_or_else(today);
            let o = store.sell_position(&code, price, quantity, &date, note.as_deref())?;
            let pct = if o.avg_cost != 0.0 {
                (price - o.avg_cost) / o.avg_cost * 100.0
            } else {
                0.0
            };
            println!(
                "已卖出 {} {}股 @ {:.3}  成本 ¥{:.3}  已实现盈亏 ¥{} ({:+.2}%)",
                code,
                o.sold_qty,
                price,
                o.avg_cost,
                money(o.realized_pnl),
                pct
            );
            if o.remaining_qty == 0 {
                println!("已清仓 {}", code);
            } else {
                println!("剩余持仓 {}股", o.remaining_qty);
            }
            Ok(())
        }
        PortfolioCmd::Remove { code } => {
            if store.remove_position(&code)? {
                println!("已移除持仓 {}", code);
            } else {
                println!("{} 无持仓", code);
            }
            Ok(())
        }
        PortfolioCmd::List => dashboard(&store).await,
        PortfolioCmd::History => history(&store),
        PortfolioCmd::Stats {
            code,
            all,
            benchmark,
        } => stats(&mut store, code, all, benchmark.as_deref()).await,
        PortfolioCmd::Cash { amount } => {
            match amount {
                Some(a) => {
                    if !a.is_finite() || a < 0.0 {
                        return Err(anyhow!("现金金额无效:{}", a));
                    }
                    store.set_cash(a)?;
                    println!("现金余额已设为 ¥{}", money(a));
                }
                None => match store.get_cash()? {
                    Some(a) => println!("现金余额 ¥{}", money(a)),
                    None => println!("未设置现金余额(portfolio cash <金额> 设置)"),
                },
            }
            Ok(())
        }
    }
}

/// 批量拉全量实时行情,按代码索引。取数失败时返回空表,调用方降级为买入价。
async fn quote_map(codes: &[String]) -> HashMap<String, Quote> {
    let mut reqs: Vec<(String, Market)> = Vec::new();
    for c in codes {
        if let Some(cm) = normalize_code(c) {
            if !reqs.iter().any(|(rc, _)| rc == &cm.0) {
                reqs.push(cm);
            }
        }
    }
    let by_norm: HashMap<String, Quote> = source::fetch_quotes(&reqs)
        .await
        .map(|(qs, _)| qs.into_iter().map(|(q, _)| (q.code.clone(), q)).collect())
        .unwrap_or_default();
    // 按调用方原始代码重新索引:HK 未补零代码(如 "700")才能命中规范化行情("00700")。
    let mut out = HashMap::new();
    for c in codes {
        if let Some((norm, _)) = normalize_code(c) {
            if let Some(q) = by_norm.get(&norm) {
                out.insert(c.clone(), q.clone());
            }
        }
    }
    out
}

fn print_realized(store: &Store, code: Option<&str>) -> Result<()> {
    let realized = store.realized_pnl(code)?;
    if realized != 0.0 {
        println!("已实现盈亏 ¥{}", money(realized));
    }
    Ok(())
}

async fn dashboard(store: &Store) -> Result<()> {
    let positions = store.list_positions()?;
    if positions.is_empty() {
        println!("当前无持仓");
        if let Some(cash) = store.get_cash()? {
            println!("现金 ¥{}  总资产 ¥{}", money(cash), money(cash));
        }
        return Ok(());
    }
    let codes: Vec<String> = positions.iter().map(|p| p.code.clone()).collect();
    let quotes = quote_map(&codes).await;

    let mut table = Table::new();
    table.set_header(vec![
        "代码", "数量", "现价", "今日%", "今日盈亏", "市值", "浮动盈亏", "浮动%",
    ]);

    let mut total_cost = 0.0;
    let mut total_value = 0.0;
    let mut today_pnl = 0.0;
    let mut prev_value = 0.0; // 昨收市值,用于今日涨跌%
    for p in &positions {
        let q = quotes.get(&p.code);
        let cur = q.map(|q| q.price).unwrap_or(p.price);
        let qf = p.quantity as f64;
        let cost = p.price * qf;
        let value = cur * qf;
        let pnl = value - cost;
        let pnl_pct = if cost != 0.0 { pnl / cost } else { 0.0 };
        // 今日盈亏 = 每股涨跌额 × 持股。今日口径只累计有实时行情的持仓——无行情者
        // 既不计入今日盈亏,也不计入昨收市值(分母),否则会用买入价冒充昨收稀释今日%。
        let today_pos = q.map(|q| q.change * qf);
        total_cost += cost;
        total_value += value;
        if let Some(q) = q {
            today_pnl += q.change * qf;
            prev_value += q.prev_close * qf;
        }
        table.add_row(vec![
            p.code.clone(),
            p.quantity.to_string(),
            format!("{:.3}", cur),
            q.map_or("--".into(), |q| format!("{:+.2}%", q.change_pct * 100.0)),
            today_pos.map_or("--".into(), money),
            money(value),
            money(pnl),
            format!("{:+.2}%", pnl_pct * 100.0),
        ]);
    }
    println!("{table}");
    println!("{}", "─".repeat(52));

    let total_pnl = total_value - total_cost;
    let total_pct = if total_cost != 0.0 {
        total_pnl / total_cost
    } else {
        0.0
    };
    let today_pct = if prev_value != 0.0 {
        today_pnl / prev_value
    } else {
        0.0
    };
    println!(
        "今日盈亏 ¥{} ({:+.2}%)   累计浮动 ¥{} ({:+.2}%)",
        money(today_pnl),
        today_pct * 100.0,
        money(total_pnl),
        total_pct * 100.0
    );
    // 第二行:已实现 / 总市值 [/ 现金 / 总资产]
    let realized = store.realized_pnl(None)?;
    let mut line = String::new();
    if realized != 0.0 {
        line.push_str(&format!("已实现 ¥{}   ", money(realized)));
    }
    line.push_str(&format!("总市值 ¥{}", money(total_value)));
    if let Some(cash) = store.get_cash()? {
        line.push_str(&format!(
            "   现金 ¥{}   总资产 ¥{}",
            money(cash),
            money(total_value + cash)
        ));
    }
    println!("{line}");
    Ok(())
}

fn history(store: &Store) -> Result<()> {
    let trades = store.list_trades()?;
    if trades.is_empty() {
        println!("无交易记录");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec![
        "日期",
        "代码",
        "方向",
        "价格",
        "数量",
        "成本",
        "已实现盈亏",
        "备注",
    ]);
    for t in trades {
        let action = if t.action == "buy" {
            "买入"
        } else if t.action == "sell" {
            "卖出"
        } else {
            &t.action
        };
        table.add_row(vec![
            t.date,
            t.code,
            action.to_string(),
            format!("{:.3}", t.price),
            t.quantity.to_string(),
            t.cost_basis.map_or("--".into(), |c| format!("{:.3}", c)),
            t.pnl.map_or("--".into(), money),
            t.note.unwrap_or_default(),
        ]);
    }
    println!("{table}");
    print_realized(store, None)?;
    Ok(())
}

async fn stats(
    store: &mut Store,
    code: Option<String>,
    all: bool,
    benchmark_override: Option<&str>,
) -> Result<()> {
    // 持仓一次读取,供目标筛选与各标的聚合复用(避免 --all 时 N+1 次全表扫描)。
    let all_positions = store.list_positions()?;
    // 目标集合:显式单只;--all 或省略代码 → 全部持仓(去重,保持列表顺序)。
    let targets: Vec<String> = match code {
        Some(c) if !all => vec![c],
        _ => {
            let mut seen = Vec::new();
            for p in &all_positions {
                if !seen.contains(&p.code) {
                    seen.push(p.code.clone());
                }
            }
            seen
        }
    };
    if targets.is_empty() {
        return Err(anyhow!("当前无持仓,无可分析标的"));
    }
    let quotes = quote_map(&targets).await;
    let multi = targets.len() > 1;
    for (i, c) in targets.iter().enumerate() {
        if multi && i > 0 {
            println!("\n{}", "═".repeat(48));
        }
        if let Err(e) = stats_one(store, c, &all_positions, quotes.get(c), benchmark_override).await
        {
            // --all 时单只失败(如缺本地日K)不致命,提示并继续(错误信息已含代码)。
            if multi {
                println!("跳过 {}", e);
            } else {
                return Err(e);
            }
        }
    }
    Ok(())
}

async fn stats_one(
    store: &mut Store,
    code: &str,
    all_positions: &[Position],
    quote: Option<&Quote>,
    benchmark_override: Option<&str>,
) -> Result<()> {
    // 聚合该代码的在场批次：总量、加权成本、最早建仓日(从已读入的持仓过滤,不再查库)。
    let lots = || all_positions.iter().filter(|p| p.code == code);
    let qty: i64 = lots().map(|p| p.quantity).sum();
    let realized = store.realized_pnl(Some(code))?;
    if qty == 0 {
        // 已清仓/未持有:有已实现盈亏则展示(明确标注是本股),否则报错(含代码,便于定位笔误)。
        if realized != 0.0 {
            let name = store.get_stock(code)?.map(|s| s.name).unwrap_or_default();
            println!(
                "{} {} 已清仓  本股已实现盈亏 ¥{}",
                code,
                name,
                money(realized)
            );
            return Ok(());
        }
        return Err(anyhow!("{} 无持仓", code));
    }
    let cost: f64 = lots().map(|p| p.price * p.quantity as f64).sum();
    let avg_cost = cost / qty as f64;
    let buy_date = lots().map(|p| p.date.clone()).min().unwrap();

    let name = store.get_stock(code)?.map(|s| s.name).unwrap_or_default();
    let klines = store.get_klines(code, Period::Day, Some(&buy_date), None)?;
    if klines.is_empty() {
        return Err(anyhow!(
            "建仓日 {} 起无本地日K,请先 stockrs data update {}",
            buy_date,
            code
        ));
    }
    let dates: Vec<String> = klines.iter().map(|k| k.date.clone()).collect();
    let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let s = position_stats(avg_cost, qty, &dates, &closes);
    let last_date = dates.last().unwrap().clone();
    let cal_days = days_since(&buy_date).unwrap_or(0);

    // 现价优先用实时行情,缺失退回本地日K收盘;市值/浮盈按现价算,避免旧价错账。
    let (cur, cur_value, cur_pnl) = match quote {
        Some(q) => {
            let v = q.price * qty as f64;
            (q.price, v, v - s.cost)
        }
        None => (s.last_close, s.value, s.pnl),
    };
    let cur_pct = if s.cost != 0.0 { cur_pnl / s.cost } else { 0.0 };

    let ret = |o: Option<f64>| o.map_or("--".to_string(), |v| format!("{:+.2}%", v * 100.0));

    println!("{} {} 持仓分析", code, name);
    println!(
        "建仓 {}（{} 自然日 / {} 交易日）",
        buy_date, cal_days, s.trading_days
    );
    println!("成本 ¥{:.3} × {} = ¥{}", s.avg_cost, s.qty, money(s.cost));
    match quote {
        Some(q) => println!(
            "现价 ¥{:.3} ({:+.2}% 今日)   市值 ¥{}",
            cur,
            q.change_pct * 100.0,
            money(cur_value)
        ),
        None => println!(
            "现价 ¥{:.3}（本地日K {} 收盘,无实时价）   市值 ¥{}",
            cur, last_date, money(cur_value)
        ),
    }
    // 数据滞后陷阱:本地日K明显落后(>5 自然日,跳过周末/短假的假警报)时提醒更新。
    // 市值已优先用实时价,此提示主要针对下方的收益曲线/极值/基准同期对比。
    if days_since(&last_date).unwrap_or(0) > 5 {
        println!(
            "⚠ 本地日K截至 {},可能滞后;最新收盘请 stockrs data update {}",
            last_date, code
        );
    }
    println!("{}", "─".repeat(46));
    println!(
        "浮动盈亏：¥{} ({:+.2}%)   日均 ¥{}/交易日",
        money(cur_pnl),
        cur_pct * 100.0,
        money(s.avg_daily_pnl)
    );
    println!(
        "收益率：  今日 {}   近一周 {}   近一月 {}   累计 {:+.2}%",
        quote.map_or_else(
            || ret(s.ret_day),
            |q| format!("{:+.2}%", q.change_pct * 100.0)
        ),
        ret(s.ret_week),
        ret(s.ret_month),
        cur_pct * 100.0
    );

    // 基准对比(建仓至今、按收盘对齐):默认按市场自动选,--benchmark 覆盖;港股/取数失败则跳过。
    let market = normalize_code(code).map(|(_, m)| m);
    let bench_alias =
        benchmark_override.or_else(|| market.and_then(|m| benchmark::benchmark_for(code, m)));
    if let Some(alias) = bench_alias {
        match benchmark::fetch(store, alias, Some(&buy_date), Some(&last_date)).await {
            Some((_, bname, bks)) if bks.len() >= 2 => {
                // 本股回报自持仓成本计(与上方浮盈同口径),基准自建仓日收盘计(同期"若改买指数");
                // 标签写明两侧口径,避免把成本基回报误当收盘基。
                let idx_ret = bks.last().unwrap().close / bks.first().unwrap().close - 1.0;
                let excess = s.pnl_pct - idx_ret;
                let verdict = if excess >= 0.0 { "跑赢" } else { "跑输" };
                println!(
                    "vs {}(建仓至今)：{} {:+.2}%（本股 {:+.2}% 自成本 / 基准 {:+.2}% 同期收盘）",
                    bname,
                    verdict,
                    excess * 100.0,
                    s.pnl_pct * 100.0,
                    idx_ret * 100.0
                );
            }
            _ => println!("（基准 {} 数据不可用,跳过对比）", alias),
        }
    }

    // 极值/回撤/曲线:交易日太少(<MIN_DAYS_FOR_CURVE)时样本无意义,略去。
    if s.trading_days >= MIN_DAYS_FOR_CURVE {
        println!("{}", "─".repeat(46));
        println!(
            "最大浮盈：¥{} ({:+.2}%) @ {}",
            money(s.max_profit.0),
            s.max_profit.1 * 100.0,
            s.max_profit.2
        );
        println!(
            "最大浮亏：¥{} ({:+.2}%) @ {}",
            money(s.max_loss.0),
            s.max_loss.1 * 100.0,
            s.max_loss.2
        );
        println!(
            "最大回撤：{:.2}%（持仓期市值峰值→谷值）",
            s.max_drawdown * 100.0
        );
        println!("收益曲线(浮盈%)：");
        println!("{}", sparkline(&s.pnl_pct_series, 46));
    } else {
        println!(
            "（持仓 {} 交易日,样本太短,略去极值/回撤/曲线）",
            s.trading_days
        );
    }

    if realized != 0.0 {
        println!("本股已实现盈亏 ¥{}", money(realized));
    }
    Ok(())
}
