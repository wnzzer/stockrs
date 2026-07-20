use crate::utils::date::{days_since, today};
use anyhow::{anyhow, Result};
use clap::Subcommand;
use comfy_table::Table;

use crate::data::models::normalize_code;
use crate::data::source;
use crate::data::Store;
use crate::engine::position_stats;
use crate::utils::format::{money, sparkline};

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
    /// 移除持仓
    Remove { code: String },
    /// 当前持仓 + 实时盈亏
    List,
    /// 历史交易记录
    History,
    /// 持仓收益分析（收益曲线、回撤、日均收益等）
    Stats { code: String },
}

pub async fn run(cmd: PortfolioCmd) -> Result<()> {
    let store = Store::open_default()?;
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
        PortfolioCmd::Remove { code } => {
            if store.remove_position(&code)? {
                println!("已移除持仓 {}", code);
            } else {
                println!("{} 无持仓", code);
            }
            Ok(())
        }
        PortfolioCmd::List => list(&store).await,
        PortfolioCmd::History => history(&store),
        PortfolioCmd::Stats { code } => stats(&store, &code),
    }
}

async fn list(store: &Store) -> Result<()> {
    let positions = store.list_positions()?;
    if positions.is_empty() {
        println!("当前无持仓");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec![
        "代码",
        "买入价",
        "数量",
        "现价",
        "市值",
        "盈亏",
        "盈亏%",
        "买入日",
    ]);
    // 一次批量拉取所有持仓的实时价，避免逐只请求
    let reqs: Vec<(String, crate::data::Market)> = positions
        .iter()
        .filter_map(|p| normalize_code(&p.code))
        .collect();
    let prices: std::collections::HashMap<String, f64> = source::fetch_quotes(&reqs)
        .await
        .map(|(qs, _)| qs.into_iter().map(|(q, _)| (q.code, q.price)).collect())
        .unwrap_or_default();

    let mut total_cost = 0.0;
    let mut total_value = 0.0;
    for p in positions {
        let cur = prices.get(&p.code).copied().unwrap_or(p.price);
        let cost = p.price * p.quantity as f64;
        let value = cur * p.quantity as f64;
        let pnl = value - cost;
        let pnl_pct = if cost != 0.0 { pnl / cost } else { 0.0 };
        total_cost += cost;
        total_value += value;
        table.add_row(vec![
            p.code,
            format!("{:.2}", p.price),
            p.quantity.to_string(),
            format!("{:.2}", cur),
            money(value),
            money(pnl),
            format!("{:+.2}%", pnl_pct * 100.0),
            p.date,
        ]);
    }
    println!("{table}");
    let total_pnl = total_value - total_cost;
    let total_pct = if total_cost != 0.0 {
        total_pnl / total_cost
    } else {
        0.0
    };
    println!(
        "合计市值 ¥{}  盈亏 ¥{} ({:+.2}%)",
        money(total_value),
        money(total_pnl),
        total_pct * 100.0
    );
    Ok(())
}

fn history(store: &Store) -> Result<()> {
    let trades = store.list_trades()?;
    if trades.is_empty() {
        println!("无交易记录");
        return Ok(());
    }
    let mut table = Table::new();
    table.set_header(vec!["日期", "代码", "方向", "价格", "数量", "备注"]);
    for t in trades {
        table.add_row(vec![
            t.date,
            t.code,
            t.action,
            format!("{:.2}", t.price),
            t.quantity.to_string(),
            t.note.unwrap_or_default(),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn stats(store: &Store, code: &str) -> Result<()> {
    // 聚合该代码的所有持仓：总量、加权成本、最早建仓日
    let positions: Vec<_> = store
        .list_positions()?
        .into_iter()
        .filter(|p| p.code == code)
        .collect();
    if positions.is_empty() {
        return Err(anyhow!("{} 无持仓", code));
    }
    let qty: i64 = positions.iter().map(|p| p.quantity).sum();
    let cost: f64 = positions.iter().map(|p| p.price * p.quantity as f64).sum();
    let avg_cost = if qty != 0 { cost / qty as f64 } else { 0.0 };
    let buy_date = positions.iter().map(|p| p.date.clone()).min().unwrap();

    let name = store.get_stock(code)?.map(|s| s.name).unwrap_or_default();
    let klines = store.get_klines(code, crate::data::Period::Day, Some(&buy_date), None)?;
    if klines.is_empty() {
        return Err(anyhow!(
            "{} 建仓日 {} 起无本地日K，请先 data add / update {}",
            code,
            buy_date,
            code
        ));
    }
    let dates: Vec<String> = klines.iter().map(|k| k.date.clone()).collect();
    let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let s = position_stats(avg_cost, qty, &dates, &closes);

    let cal_days = days_since(&buy_date).unwrap_or(0);

    let ret = |o: Option<f64>| o.map_or("--".to_string(), |v| format!("{:+.2}%", v * 100.0));

    println!("{} {} 持仓分析", code, name);
    println!(
        "建仓 {}（持仓 {} 自然日 / {} 交易日，数据截至 {}）",
        buy_date,
        cal_days,
        s.trading_days,
        dates.last().unwrap()
    );
    println!("成本 ¥{:.3} × {} = ¥{}", s.avg_cost, s.qty, money(s.cost));
    println!("最新 ¥{:.3} → 市值 ¥{}", s.last_close, money(s.value));
    println!("{}", "─".repeat(46));
    println!(
        "浮动盈亏：¥{} ({:+.2}%)   日均 ¥{}/交易日",
        money(s.pnl),
        s.pnl_pct * 100.0,
        money(s.avg_daily_pnl)
    );
    println!(
        "收益率：  今日 {}   近一周 {}   近一月 {}   累计 {:+.2}%",
        ret(s.ret_day),
        ret(s.ret_week),
        ret(s.ret_month),
        s.pnl_pct * 100.0
    );
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
    println!("{}", "─".repeat(46));
    println!("收益曲线(浮盈%)：");
    println!("{}", sparkline(&s.pnl_pct_series, 46));
    Ok(())
}
