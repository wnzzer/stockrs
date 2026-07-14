use anyhow::Result;
use chrono::Local;
use clap::Subcommand;
use comfy_table::Table;

use crate::data::models::infer_market;
use crate::data::source;
use crate::data::Store;
use crate::utils::format::money;

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
            let date = date.unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
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
        .filter_map(|p| infer_market(&p.code).map(|m| (p.code.clone(), m)))
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
