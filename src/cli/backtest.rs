use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use comfy_table::Table;
use rhai::Scope;

use crate::data::benchmark;
use crate::data::models::normalize_code;
use crate::data::{KLine, Market, Store};
use crate::engine::context::TradeRec;
use crate::engine::metrics::{Benchmark, Metrics};
use crate::engine::portfolio::{self, PfTrade, PortfolioCtx, PortfolioResult, StockData};
use crate::engine::{self, Ctx};
use crate::strategy::Strategy;
use crate::utils::format::{money, pad_end};

/// 参数组合数上限，避免网格爆炸。
const MAX_COMBOS: usize = 200;
/// 报告中最多显示的成交明细行数。
const MAX_TRADES_SHOWN: usize = 40;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    script: String,
    stock: Option<String>,
    stocks: Vec<String>,
    universe: bool,
    start: Option<String>,
    end: Option<String>,
    capital: f64,
    benchmark: Option<String>,
    param: Vec<String>,
    optimize: Option<String>,
) -> Result<()> {
    let mut store = Store::open_default()?;
    let grid = parse_param_grid(&param)?;
    let combos = cartesian(&grid);
    if combos.len() > MAX_COMBOS {
        bail!(
            "参数组合数 {} 超过上限 {}，请减少参数取值",
            combos.len(),
            MAX_COMBOS
        );
    }

    let is_portfolio = universe || !stocks.is_empty();
    if is_portfolio && stock.is_some() {
        bail!("--stock 与 --stocks/--universe 互斥，请二选一");
    }

    if is_portfolio {
        let codes: Vec<String> = if universe {
            store.list_stocks()?.into_iter().map(|s| s.code).collect()
        } else {
            stocks
        };
        if codes.is_empty() {
            bail!("组合为空：--universe 需先 data add 跟踪股票，或用 --stocks 指定代码");
        }
        run_portfolio(
            &mut store, &script, codes, start, end, capital, benchmark, &grid, combos, optimize,
        )
        .await
    } else {
        let code = stock.ok_or_else(|| {
            anyhow!("请用 --stock <代码> 指定单标的，或用 --stocks/--universe 做组合回测")
        })?;
        run_single(
            &mut store, &script, code, start, end, capital, benchmark, &grid, combos, optimize,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_single(
    store: &mut Store,
    script: &str,
    code: String,
    start: Option<String>,
    end: Option<String>,
    capital: f64,
    benchmark: Option<String>,
    grid: &[(String, Vec<f64>)],
    combos: Vec<HashMap<String, f64>>,
    optimize: Option<String>,
) -> Result<()> {
    let (code, market) = normalize_code(&code).ok_or_else(|| anyhow!("无法识别的代码 {}", code))?;
    if market == Market::HK {
        eprintln!(
            "⚠️ 港股回测暂用 A 股规则(每手 100 股 / A 股费率),数字仅供参考;正确港股规则见后续版本"
        );
    }
    let name = store
        .get_stock(&code)?
        .map(|s| s.name)
        .unwrap_or_else(|| code.clone());
    let klines = store.get_klines(&code, start.as_deref(), end.as_deref())?;
    if klines.len() < 2 {
        bail!("{} 本地数据不足，请先 data add / update", code);
    }
    // 基本面(缺失则全 NaN,不影响回测);参数扫描各 combo 复用同一份。
    let funda = store.get_fundamentals(&code, start.as_deref(), end.as_deref())?;
    let dates: Vec<String> = klines.iter().map(|k| k.date.clone()).collect();
    let period_start = dates.first().cloned().unwrap_or_default();
    let period_end = dates.last().cloned().unwrap_or_default();
    let strategy = Strategy::load(script)?;
    let strat_name = strategy.name();

    if grid.is_empty() {
        let ctx = Ctx::new(klines, capital, &funda);
        let result = run_single_ctx(&ctx, &strategy)?;
        let strat_equity = ctx.equity_curve();
        let bench = fetch_bench_equity(
            store,
            benchmark.as_deref(),
            &dates,
            capital,
            start.as_deref(),
            end.as_deref(),
        )
        .await;
        let bench_stats =
            bench.map(|(n, c, be)| engine::compute_benchmark(n, c, &strat_equity, &be));
        print_single_report(
            &strat_name,
            &code,
            &name,
            &period_start,
            &period_end,
            &result,
            bench_stats.as_ref(),
        );
    } else {
        let mut rows: Vec<(HashMap<String, f64>, Metrics)> = Vec::new();
        for combo in combos {
            let ctx = Ctx::new_with_params(klines.clone(), capital, combo.clone(), &funda);
            let result = run_single_ctx(&ctx, &strategy)?;
            rows.push((combo, result.metrics));
        }
        print_scan_report(
            &strat_name,
            &format!("{} {}", code, name),
            &period_start,
            &period_end,
            grid,
            rows,
            optimize.as_deref(),
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_portfolio(
    store: &mut Store,
    script: &str,
    codes: Vec<String>,
    start: Option<String>,
    end: Option<String>,
    capital: f64,
    benchmark: Option<String>,
    grid: &[(String, Vec<f64>)],
    combos: Vec<HashMap<String, f64>>,
    optimize: Option<String>,
) -> Result<()> {
    let mut data: Vec<StockData> = Vec::new();
    let mut has_hk = false;
    for input in &codes {
        let Some((code, market)) = normalize_code(input) else {
            eprintln!("跳过 {}：无法识别的代码", input);
            continue;
        };
        if market == Market::HK {
            has_hk = true;
        }
        let name = store
            .get_stock(&code)?
            .map(|s| s.name)
            .unwrap_or_else(|| code.clone());
        let ks = store.get_klines(&code, start.as_deref(), end.as_deref())?;
        if ks.len() < 2 {
            eprintln!("跳过 {}：本地数据不足", code);
            continue;
        }
        let funda = store.get_fundamentals(&code, start.as_deref(), end.as_deref())?;
        data.push(StockData {
            code,
            name,
            klines: ks,
            funda,
        });
    }
    if has_hk {
        eprintln!(
            "⚠️ 港股回测暂用 A 股规则(每手 100 股 / A 股费率),数字仅供参考;正确港股规则见后续版本"
        );
    }
    if data.is_empty() {
        bail!("组合中无可用股票数据，请先 data add / update");
    }
    let strategy = Strategy::load_portfolio(script)?;
    let strat_name = strategy.name();

    if grid.is_empty() {
        let ctx = PortfolioCtx::new(data, capital, HashMap::new());
        let dates = ctx.dates();
        let result = run_portfolio_ctx(&ctx, &strategy)?;
        let bench = fetch_bench_equity(
            store,
            benchmark.as_deref(),
            &dates,
            capital,
            start.as_deref(),
            end.as_deref(),
        )
        .await;
        let bench_stats =
            bench.map(|(n, c, be)| engine::compute_benchmark(n, c, &result.equity, &be));
        print_portfolio_report(&strat_name, &result, bench_stats.as_ref());
    } else {
        let (period_start, period_end) = period_of(&data);
        let mut rows: Vec<(HashMap<String, f64>, Metrics)> = Vec::new();
        for combo in combos {
            let ctx = PortfolioCtx::new(data.clone(), capital, combo.clone());
            let result = run_portfolio_ctx(&ctx, &strategy)?;
            rows.push((combo, result.metrics));
        }
        print_scan_report(
            &strat_name,
            &format!("组合 {} 只", data.len()),
            &period_start,
            &period_end,
            grid,
            rows,
            optimize.as_deref(),
        );
    }
    Ok(())
}

fn run_single_ctx(ctx: &Ctx, strategy: &Strategy) -> Result<engine::BacktestResult> {
    let mut scope = Scope::new();
    let ctx_ref = ctx;
    let strategy_ref = strategy;
    let scope_ref = &mut scope;
    engine::run(ctx_ref, || {
        strategy_ref.call_on_bar(scope_ref, ctx_ref.clone())
    })
}

fn run_portfolio_ctx(ctx: &PortfolioCtx, strategy: &Strategy) -> Result<PortfolioResult> {
    let mut scope = Scope::new();
    let ctx_ref = ctx;
    let strategy_ref = strategy;
    let scope_ref = &mut scope;
    portfolio::run(ctx_ref, || {
        strategy_ref.call_on_bar_pf(scope_ref, ctx_ref.clone())
    })
}

fn period_of(data: &[StockData]) -> (String, String) {
    let mut dates: Vec<&String> = data
        .iter()
        .flat_map(|sd| sd.klines.iter().map(|k| &k.date))
        .collect();
    dates.sort();
    let ps = dates.first().map(|s| s.to_string()).unwrap_or_default();
    let pe = dates.last().map(|s| s.to_string()).unwrap_or_default();
    (ps, pe)
}

// ---- 基准 ----

/// 取基准并对齐到回测日期轴，产出与策略等长的基准资金曲线。失败则告警并返回 None。
async fn fetch_bench_equity(
    store: &mut Store,
    alias_opt: Option<&str>,
    dates: &[String],
    initial: f64,
    start: Option<&str>,
    end: Option<&str>,
) -> Option<(String, String, Vec<f64>)> {
    let alias = alias_opt?;
    match benchmark::fetch(store, alias, start, end).await {
        Some((code, name, ks)) => match align_benchmark(dates, &ks, initial) {
            Some(be) => Some((name, code, be)),
            None => {
                eprintln!("基准 {} 数据无法对齐，跳过基准对比", name);
                None
            }
        },
        None => {
            eprintln!("基准 {} 数据不可用，跳过基准对比", alias);
            None
        }
    }
}

/// 把基准日K对齐到 master 日期轴并归一到初始资金。
/// 缺失日 carry-forward；首段缺失用第一个有效值后向填充；基点锚第一个有效值。
fn align_benchmark(dates: &[String], bench: &[KLine], initial: f64) -> Option<Vec<f64>> {
    if dates.is_empty() {
        return None;
    }
    let map: HashMap<&str, f64> = bench.iter().map(|k| (k.date.as_str(), k.close)).collect();
    let mut aligned: Vec<Option<f64>> = Vec::with_capacity(dates.len());
    let mut last: Option<f64> = None;
    for d in dates {
        if let Some(&c) = map.get(d.as_str()) {
            last = Some(c);
        }
        aligned.push(last);
    }
    let first_valid = aligned.iter().find_map(|x| *x)?;
    for slot in aligned.iter_mut() {
        if slot.is_none() {
            *slot = Some(first_valid);
        }
    }
    let base = aligned[0]?;
    if base <= 0.0 {
        return None;
    }
    Some(
        aligned
            .iter()
            .map(|x| initial * x.unwrap() / base)
            .collect(),
    )
}

// ---- 参数网格 ----

fn parse_param_grid(params: &[String]) -> Result<Vec<(String, Vec<f64>)>> {
    let mut out = Vec::new();
    for p in params {
        let (k, vs) = p
            .split_once('=')
            .ok_or_else(|| anyhow!("参数格式应为 key=v1,v2，收到：{}", p))?;
        let vals: Result<Vec<f64>> = vs
            .split(',')
            .map(|x| {
                x.trim()
                    .parse::<f64>()
                    .map_err(|_| anyhow!("参数值不是数字：{}", x))
            })
            .collect();
        let vals = vals?;
        if vals.is_empty() {
            bail!("参数 {} 没有取值", k);
        }
        out.push((k.trim().to_string(), vals));
    }
    Ok(out)
}

/// 展开为参数组合的笛卡尔积。空网格返回单个空组合（普通单跑）。
fn cartesian(grid: &[(String, Vec<f64>)]) -> Vec<HashMap<String, f64>> {
    let mut combos = vec![HashMap::new()];
    for (k, vals) in grid {
        let mut next = Vec::with_capacity(combos.len() * vals.len());
        for c in &combos {
            for v in vals {
                let mut m = c.clone();
                m.insert(k.clone(), *v);
                next.push(m);
            }
        }
        combos = next;
    }
    combos
}

fn opt_key(m: &Metrics, key: &str) -> f64 {
    match key {
        "sharpe" => m.sharpe,
        "annual" => m.annual_return,
        // max_drawdown 存为负数，降序即“最接近 0（回撤最小）”排前。
        "drawdown" => m.max_drawdown,
        _ => m.total_return,
    }
}

// ---- 报告 ----

const INNER: usize = 48;

fn print_line(content: &str) {
    println!("│ {}│", pad_end(content, INNER - 1));
}

fn row(label: &str, value: String) {
    print_line(&format!("{}：{}", label, value));
}

fn hr() {
    println!("├{}┤", "─".repeat(INNER));
}

fn fmt_num2(v: f64) -> String {
    if v.is_finite() {
        format!("{:.2}", v)
    } else {
        "—".to_string()
    }
}

fn fmt_pct2(v: f64) -> String {
    if v.is_finite() {
        format!("{:+.2}%", v * 100.0)
    } else {
        "—".to_string()
    }
}

fn print_metrics_rows(m: &Metrics) {
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
}

fn print_benchmark_rows(b: &Benchmark) {
    row("基准", format!("{} {}", b.code, b.name));
    row("基准收益", format!("{:+.2}%", b.total_return * 100.0));
    row("超额收益", format!("{:+.2}%", b.excess * 100.0));
    row("Beta", fmt_num2(b.beta));
    row("年化Alpha", fmt_pct2(b.alpha_annual));
}

fn print_single_report(
    strat: &str,
    code: &str,
    name: &str,
    start: &str,
    end: &str,
    result: &engine::BacktestResult,
    bench: Option<&Benchmark>,
) {
    let line = "─".repeat(INNER);
    println!("┌{}┐", line);
    print_line(&format!("回测报告：{}", strat));
    hr();
    row("股票", format!("{} {}", code, name));
    row("区间", format!("{} ~ {}", start, end));
    print_metrics_rows(&result.metrics);
    if let Some(b) = bench {
        hr();
        print_benchmark_rows(b);
    }
    hr();
    print_line("交易明细：");
    print_single_trades(&result.trades);
    println!("└{}┘", line);
}

fn print_single_trades(trades: &[TradeRec]) {
    let n = trades.len();
    for (i, t) in trades.iter().enumerate() {
        if i >= MAX_TRADES_SHOWN {
            print_line(&format!("… 还有 {} 笔", n - MAX_TRADES_SHOWN));
            break;
        }
        let pnl = match t.pnl_pct {
            Some(p) => format!("{:+.2}%", p * 100.0),
            None => String::new(),
        };
        print_line(&format!(
            "{}  {:<4} {} @ ¥{:.2}  {}",
            t.date, t.action, t.shares, t.price, pnl
        ));
    }
}

fn print_portfolio_report(strat: &str, result: &PortfolioResult, bench: Option<&Benchmark>) {
    let line = "─".repeat(INNER);
    let start = result.dates.first().cloned().unwrap_or_default();
    let end = result.dates.last().cloned().unwrap_or_default();
    println!("┌{}┐", line);
    print_line(&format!("组合回测：{}", strat));
    hr();
    row("股票池", format!("{} 只", result.stock_count));
    row("区间", format!("{} ~ {}", start, end));
    print_metrics_rows(&result.metrics);
    if let Some(b) = bench {
        hr();
        print_benchmark_rows(b);
    }
    hr();
    print_line("期末持仓：");
    if result.holdings.is_empty() {
        print_line("（空仓）");
    }
    for h in &result.holdings {
        print_line(&format!(
            "{} {}  {}股 @¥{:.2}  现¥{:.2} 市值¥{:.0}  {:+.2}%",
            h.code,
            h.name,
            h.shares,
            h.avg_cost,
            h.last_price,
            h.value,
            h.pnl_pct * 100.0
        ));
    }
    hr();
    print_line(&format!("交易明细：共 {} 笔", result.trades.len()));
    print_pf_trades(&result.trades);
    println!("└{}┘", line);
}

fn print_pf_trades(trades: &[PfTrade]) {
    let n = trades.len();
    for (i, t) in trades.iter().enumerate() {
        if i >= MAX_TRADES_SHOWN {
            print_line(&format!("… 还有 {} 笔", n - MAX_TRADES_SHOWN));
            break;
        }
        let pnl = match t.pnl_pct {
            Some(p) => format!("{:+.2}%", p * 100.0),
            None => String::new(),
        };
        print_line(&format!(
            "{}  {:<4} {} {}股 @¥{:.2}  {}",
            t.date, t.action, t.code, t.shares, t.price, pnl
        ));
    }
}

fn print_scan_report(
    strat: &str,
    subject: &str,
    start: &str,
    end: &str,
    grid: &[(String, Vec<f64>)],
    mut rows: Vec<(HashMap<String, f64>, Metrics)>,
    optimize: Option<&str>,
) {
    let key = optimize.unwrap_or("return");
    rows.sort_by(|a, b| {
        opt_key(&b.1, key)
            .partial_cmp(&opt_key(&a.1, key))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    println!(
        "参数扫描：{}  |  {}  |  {} ~ {}  |  排序 {}",
        strat, subject, start, end, key
    );

    let mut table = Table::new();
    let mut header: Vec<String> = grid.iter().map(|(k, _)| k.clone()).collect();
    for h in ["收益%", "年化%", "回撤%", "夏普", "胜率%", "交易"] {
        header.push(h.to_string());
    }
    table.set_header(header);

    for (i, (combo, m)) in rows.iter().enumerate() {
        let mut cells: Vec<String> = grid
            .iter()
            .map(|(k, _)| fmt_param(combo.get(k).copied().unwrap_or(f64::NAN)))
            .collect();
        let star = if i == 0 { "★ " } else { "" };
        cells.push(format!("{}{:+.2}", star, m.total_return * 100.0));
        cells.push(format!("{:+.2}", m.annual_return * 100.0));
        cells.push(format!("{:.2}", m.max_drawdown * 100.0));
        cells.push(format!("{:.2}", m.sharpe));
        cells.push(format!("{:.1}", m.win_rate * 100.0));
        cells.push(m.total_trades.to_string());
        table.add_row(cells);
    }
    println!("{table}");

    if let Some((combo, m)) = rows.first() {
        println!(
            "最优参数：{}  收益 {:+.2}%  夏普 {:.2}  回撤 {:.2}%",
            fmt_combo(grid, combo),
            m.total_return * 100.0,
            m.sharpe,
            m.max_drawdown * 100.0
        );
    }
}

fn fmt_param(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 {
        (v as i64).to_string()
    } else {
        v.to_string()
    }
}

fn fmt_combo(grid: &[(String, Vec<f64>)], combo: &HashMap<String, f64>) -> String {
    grid.iter()
        .map(|(k, _)| {
            format!(
                "{}={}",
                k,
                fmt_param(combo.get(k).copied().unwrap_or(f64::NAN))
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_grid_cartesian_count() {
        let grid = parse_param_grid(&["fast=5,10".into(), "slow=20,30,60".into()]).unwrap();
        assert_eq!(grid.len(), 2);
        let combos = cartesian(&grid);
        assert_eq!(combos.len(), 6); // 2 × 3
        assert!(combos
            .iter()
            .all(|c| c.contains_key("fast") && c.contains_key("slow")));
    }

    #[test]
    fn empty_grid_yields_one_combo() {
        let grid = parse_param_grid(&[]).unwrap();
        assert!(grid.is_empty());
        assert_eq!(cartesian(&grid).len(), 1);
    }

    #[test]
    fn param_grid_rejects_bad_format() {
        assert!(parse_param_grid(&["nokv".into()]).is_err());
        assert!(parse_param_grid(&["k=abc".into()]).is_err());
    }

    #[test]
    fn benchmark_align_forward_and_backward_fill() {
        let dates: Vec<String> = ["d1", "d2", "d3", "d4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // 基准缺 d1、d3：d1 无值(后向填 d2 的 10)，d3 carry d2 的 10。
        let bench = vec![
            KLine {
                code: "b".into(),
                date: "d2".into(),
                open: 10.0,
                high: 10.0,
                low: 10.0,
                close: 10.0,
                volume: 0.0,
                amount: 0.0,
                turnover: None,
            },
            KLine {
                code: "b".into(),
                date: "d4".into(),
                open: 12.0,
                high: 12.0,
                low: 12.0,
                close: 12.0,
                volume: 0.0,
                amount: 0.0,
                turnover: None,
            },
        ];
        let eq = align_benchmark(&dates, &bench, 100_000.0).unwrap();
        assert_eq!(eq.len(), 4);
        // base = 第一个有效值 10 -> d1,d2,d3 归一为 100000，d4 为 120000。
        assert!((eq[0] - 100_000.0).abs() < 1e-6);
        assert!((eq[1] - 100_000.0).abs() < 1e-6);
        assert!((eq[2] - 100_000.0).abs() < 1e-6);
        assert!((eq[3] - 120_000.0).abs() < 1e-6);
    }

    #[test]
    fn opt_key_drawdown_prefers_closest_to_zero() {
        let mk = |dd: f64| Metrics {
            initial: 1.0,
            final_value: 1.0,
            total_return: 0.0,
            annual_return: 0.0,
            max_drawdown: dd,
            sharpe: 0.0,
            win_rate: 0.0,
            wins: 0,
            closed_trades: 0,
            total_trades: 0,
        };
        // 回撤 -0.05 优于 -0.20：降序时 -0.05 在前。
        assert!(opt_key(&mk(-0.05), "drawdown") > opt_key(&mk(-0.20), "drawdown"));
    }
}
