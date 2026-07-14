use anyhow::{anyhow, Result};
use rhai::Scope;

use crate::data::Store;
use crate::engine::{self, Ctx};
use crate::strategy::Strategy;
use crate::utils::format::{money, pad_end};

pub fn run(
    script: String,
    stock: String,
    start: Option<String>,
    end: Option<String>,
    capital: f64,
) -> Result<()> {
    let store = Store::open_default()?;
    let name = store
        .get_stock(&stock)?
        .map(|s| s.name)
        .unwrap_or_else(|| stock.clone());
    let klines = store.get_klines(&stock, start.as_deref(), end.as_deref())?;
    if klines.len() < 2 {
        return Err(anyhow!("{} 本地数据不足，请先 data add / update", stock));
    }
    let period_start = klines.first().unwrap().date.clone();
    let period_end = klines.last().unwrap().date.clone();

    let strategy = Strategy::load(&script)?;
    let strat_name = strategy.name();

    let ctx = Ctx::new(klines, capital);
    let mut scope = Scope::new();
    let result = {
        let ctx_ref = &ctx;
        let strategy_ref = &strategy;
        let scope_ref = &mut scope;
        engine::run(ctx_ref, || {
            strategy_ref.call_on_bar(scope_ref, ctx_ref.clone())
        })
    }?;

    print_report(
        &strat_name,
        &stock,
        &name,
        &period_start,
        &period_end,
        &result,
    );
    Ok(())
}

fn print_report(
    strat: &str,
    code: &str,
    name: &str,
    start: &str,
    end: &str,
    result: &engine::BacktestResult,
) {
    let m = &result.metrics;
    let line = "─".repeat(INNER);
    println!("┌{}┐", line);
    print_line(&format!("回测报告：{}", strat));
    println!("├{}┤", line);
    row("股票", format!("{} {}", code, name));
    row("区间", format!("{} ~ {}", start, end));
    row("初始资金", format!("¥{}", money(m.initial)));
    row("期末资产", format!("¥{}", money(m.final_value)));
    row("总收益", format!("{:+.2}%", m.total_return * 100.0));
    row("年化收益", format!("{:+.2}%", m.annual_return * 100.0));
    row("最大回撤", format!("{:.2}%", m.max_drawdown * 100.0));
    row("夏普比率", format!("{:.2}", m.sharpe));
    row(
        "胜率",
        format!(
            "{:.1}% ({}/{})",
            m.win_rate * 100.0,
            m.wins,
            m.closed_trades
        ),
    );
    row("成交笔数", m.total_trades.to_string());
    println!("├{}┤", line);
    print_line("交易明细：");
    for t in &result.trades {
        let pnl = match t.pnl_pct {
            Some(p) => format!("{:+.2}%", p * 100.0),
            None => String::new(),
        };
        print_line(&format!(
            "{}  {:<4} {} @ ¥{:.2}  {}",
            t.date, t.action, t.shares, t.price, pnl
        ));
    }
    println!("└{}┘", line);
}

/// 框内可用显示宽度（不含左右边框与左侧一个空格）。
const INNER: usize = 48;

fn print_line(content: &str) {
    println!("│ {}│", pad_end(content, INNER - 1));
}

fn row(label: &str, value: String) {
    print_line(&format!("{}：{}", label, value));
}
