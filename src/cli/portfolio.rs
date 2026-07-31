use crate::utils::date::{days_since, today};
use anyhow::{anyhow, Result};
use clap::{Subcommand, ValueEnum};
use comfy_table::Table;
use std::collections::HashMap;

use crate::data::models::normalize_code;
use crate::data::{benchmark, source, Market, Period, Position, Quote, Store};
use crate::engine::position_stats;
use crate::utils::format::{money, sparkline};

/// 交易日数少于此值时,极值/回撤/曲线样本太短、意义不大,略去(见持仓分析)。
const MIN_DAYS_FOR_CURVE: usize = 6;

/// 仪表盘"已清仓"区块最多列出的品种数,超出只提示总数(全部见 portfolio history)。
const MAX_CLOSED_ROWS: usize = 8;

/// 持仓盈亏的成本口径。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum CostMode {
    /// 买入成本:盈亏只看在场持仓,已实现盈亏单列(默认)
    Buy,
    /// 摊薄成本:已实现盈亏折入成本,盈亏一列即该股总盈亏(对齐东财)
    Diluted,
}

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
    List {
        /// 成本口径:buy=买入成本(默认,已实现单列);diluted=摊薄成本(已实现折入成本,对齐东财)
        #[arg(long, value_enum, default_value_t = CostMode::Buy)]
        cost_mode: CostMode,
    },
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
        PortfolioCmd::List { cost_mode } => dashboard(&store, cost_mode).await,
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

/// 今日盈亏/今日%的基准价:当日建仓用买入价,历史持仓用昨收。
/// 基准无效(≤0,如源未给出昨收)时返回 None——该持仓整条排除在今日口径外,
/// 保证今日盈亏与其分母(基准市值)始终同源。
fn today_basis(new_lot: bool, buy_price: f64, prev_close: f64) -> Option<f64> {
    let basis = if new_lot { buy_price } else { prev_close };
    (basis > 0.0).then_some(basis)
}

fn print_realized(store: &Store, code: Option<&str>) -> Result<()> {
    let realized = store.realized_pnl(code)?;
    if realized != 0.0 {
        println!("已实现盈亏 ¥{}", money(realized));
    }
    Ok(())
}

/// 仪表盘按代码聚合后的一行:多批建仓合并成一行(成本/市值求和),
/// 今日口径仍逐批算(基准价按批次的建仓日决定)再累加,与汇总同源。
struct DashRow {
    code: String,
    qty: i64,
    cost: f64,
    value: f64,
    cur: f64,
    change_pct: Option<f64>,
    /// 今日盈亏;None = 该股所有批次都无有效基准,整体排除在今日口径外。
    today_pnl: Option<f64>,
    /// 今日盈亏对应的基准市值(分母),只累计基准有效的批次。
    today_basis_value: f64,
    /// 任一批次为今日建仓(用于星号标记)。
    new_lot: bool,
    realized: f64,
}

/// 按口径算盈亏与收益率。摊薄口径把已实现盈亏折入成本(对齐东财),盈亏一列因此
/// 等于该股总盈亏;已实现吃掉全部在场成本时摊薄成本≤0,收益率失去意义,返回 None。
fn mode_pnl(mode: CostMode, cost: f64, value: f64, realized: f64) -> (f64, Option<f64>) {
    let base = match mode {
        CostMode::Buy => cost,
        CostMode::Diluted => cost - realized,
    };
    let pnl = value - base;
    (pnl, (base > 0.0).then(|| pnl / base))
}

async fn dashboard(store: &Store, cost_mode: CostMode) -> Result<()> {
    let positions = store.list_positions()?;
    if positions.is_empty() {
        println!("当前无持仓");
        if let Some(cash) = store.get_cash()? {
            println!("现金 ¥{}  总资产 ¥{}", money(cash), money(cash));
        }
        // 无在场持仓不代表没赚过:已清仓品种的收益只能从账本找回。
        print_closed(store, None)?;
        return Ok(());
    }
    let codes: Vec<String> = positions.iter().map(|p| p.code.clone()).collect();
    let quotes = quote_map(&codes).await;
    let realized_map = store.realized_by_code()?;

    // 按代码聚合(保持首次出现顺序):同一代码多批建仓合并为一行,否则"已实现"
    // 这类按代码统计的列无法归属到某个批次。
    let today_str = today();
    let mut order: Vec<String> = Vec::new();
    let mut rows: HashMap<String, DashRow> = HashMap::new();
    for p in &positions {
        let q = quotes.get(&p.code);
        let cur = q.map(|q| q.price).unwrap_or(p.price);
        let qf = p.quantity as f64;
        // 今日基准价:历史持仓用昨收。当日建仓的批次昨天并不在场,昨收→买入价这段
        // 涨跌用户没经历过,基准必须换成买入价,否则虚增当日盈亏(#2)。
        // 误录成未来日期时同样按建仓当日处理(昨收基准对它更没有意义)。
        let new_lot = p.date.as_str() >= today_str.as_str();
        // 今日口径只累计有实时行情、且基准价有效的批次——否则既不计入今日盈亏,
        // 也不计入基准市值(分母),避免用买入价冒充昨收稀释今日%。
        let basis = q.and_then(|q| today_basis(new_lot, p.price, q.prev_close));
        let row = rows.entry(p.code.clone()).or_insert_with(|| {
            order.push(p.code.clone());
            DashRow {
                code: p.code.clone(),
                qty: 0,
                cost: 0.0,
                value: 0.0,
                cur,
                change_pct: q.map(|q| q.change_pct),
                today_pnl: None,
                today_basis_value: 0.0,
                new_lot: false,
                realized: realized_map.get(&p.code).copied().unwrap_or(0.0),
            }
        });
        row.qty += p.quantity;
        row.cost += p.price * qf;
        row.value += cur * qf;
        row.new_lot |= new_lot;
        if let Some(b) = basis {
            row.today_pnl = Some(row.today_pnl.unwrap_or(0.0) + (cur - b) * qf);
            row.today_basis_value += b * qf;
        }
    }
    let rows: Vec<&DashRow> = order.iter().filter_map(|c| rows.get(c)).collect();

    let diluted = cost_mode == CostMode::Diluted;
    // 摊薄口径下"已实现"已折进盈亏列,再单列就是重复计数,故只在买入口径下加列;
    // 且全无卖出记录时不加,避免平白拉宽表格。
    let show_realized = !diluted && rows.iter().any(|r| r.realized != 0.0);

    let mut table = Table::new();
    let mut header = vec!["代码", "数量", "现价", "今日%", "今日盈亏", "市值"];
    header.extend(if diluted {
        ["总盈亏", "总%"]
    } else {
        ["浮动盈亏", "浮动%"]
    });
    if show_realized {
        header.extend(["已实现", "总盈亏"]);
    }
    table.set_header(header);

    let mut total_cost = 0.0;
    let mut total_value = 0.0;
    let mut today_pnl = 0.0;
    let mut prev_value = 0.0; // 今日基准市值,用于今日涨跌%
    let mut has_new_lot = false;
    for r in &rows {
        total_cost += r.cost;
        total_value += r.value;
        has_new_lot |= r.new_lot;
        if let Some(diff) = r.today_pnl {
            today_pnl += diff;
            prev_value += r.today_basis_value;
        }
        let (pnl, pnl_pct) = mode_pnl(cost_mode, r.cost, r.value, r.realized);
        let mut cells = vec![
            r.code.clone(),
            r.qty.to_string(),
            format!("{:.3}", r.cur),
            r.change_pct
                .map_or("--".into(), |c| format!("{:+.2}%", c * 100.0)),
            r.today_pnl.map_or("--".into(), |v| {
                // 今日建仓打星号:该行"今日%"是个股当日涨跌,盈亏却自买入价起算,
                // 不加标记会显得两列自相矛盾。
                if r.new_lot {
                    format!("{}*", money(v))
                } else {
                    money(v)
                }
            }),
            money(r.value),
            money(pnl),
            pnl_pct.map_or("--".into(), |p| format!("{:+.2}%", p * 100.0)),
        ];
        if show_realized {
            cells.push(money(r.realized));
            cells.push(money(r.value - r.cost + r.realized));
        }
        table.add_row(cells);
    }
    println!("{table}");
    if has_new_lot {
        println!("* 今日建仓,今日盈亏自买入价起算(非昨收)");
    }
    if diluted {
        println!("摊薄口径:已实现盈亏已折入成本(对齐东财),成本≤0 时收益率显示 --");
    }
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
    // 汇总行两个口径一致:始终报纯浮动 + 全账户已实现,不受 --cost-mode 影响,
    // 避免同一屏里两处"总盈亏"含义不同。
    println!(
        "今日盈亏 ¥{} ({:+.2}%)   累计浮动 ¥{} ({:+.2}%)",
        money(today_pnl),
        today_pct * 100.0,
        money(total_pnl),
        total_pct * 100.0
    );
    // 第二行:已实现 / 总盈亏 / 总市值 [/ 现金 / 总资产]
    // 已实现取全账户(含已清仓品种),故总盈亏是账户级的、不等于表内各行相加。
    let realized = store.realized_pnl(None)?;
    let mut line = String::new();
    if realized != 0.0 {
        line.push_str(&format!(
            "已实现 ¥{}   总盈亏 ¥{}   ",
            money(realized),
            money(total_pnl + realized)
        ));
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
    print_closed(store, Some(MAX_CLOSED_ROWS))?;
    Ok(())
}

/// 已清仓品种区块:这些票已从持仓表消失,收益不在这里就彻底看不到了。
/// limit=Some(n) 只列最近 n 只并提示总数,None 全列。无已清仓品种时静默跳过。
fn print_closed(store: &Store, limit: Option<usize>) -> Result<()> {
    let closed = store.closed_positions()?;
    if closed.is_empty() {
        return Ok(());
    }
    let total: f64 = closed.iter().map(|c| c.realized_pnl).sum();
    println!("\n已清仓 {} 只  已实现合计 ¥{}", closed.len(), money(total));
    let shown = limit.unwrap_or(closed.len());
    let mut table = Table::new();
    table.set_header(vec![
        "代码",
        "卖出量",
        "成本",
        "已实现",
        "收益%",
        "最后卖出",
    ]);
    for c in closed.iter().take(shown) {
        table.add_row(vec![
            c.code.clone(),
            c.sold_qty.to_string(),
            money(c.cost),
            money(c.realized_pnl),
            // 分母是各笔卖出的 FIFO 结算成本合计,与 sell 当时报的收益率同口径。
            if c.cost > 0.0 {
                format!("{:+.2}%", c.realized_pnl / c.cost * 100.0)
            } else {
                "--".into()
            },
            c.last_date.clone(),
        ]);
    }
    println!("{table}");
    if closed.len() > shown {
        println!(
            "(仅列最近 {} 只,共 {} 只;全部见 portfolio history)",
            shown,
            closed.len()
        );
    }
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
    print_closed(store, None)?;
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
            cur,
            last_date,
            money(cur_value)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 今日盈亏 = (现价 - 基准价) × 持股,与 dashboard 内的算式同构。
    fn today_pnl(new_lot: bool, buy: f64, prev_close: f64, cur: f64, qty: i64) -> Option<f64> {
        today_basis(new_lot, buy, prev_close).map(|b| (cur - b) * qty as f64)
    }

    #[test]
    fn new_lot_uses_buy_price_not_prev_close() {
        // issue #2 场景:昨收 1.61,今日大跌,用户当日 1.546 建仓,收盘 1.545。
        let (buy, prev_close, cur, qty) = (1.546, 1.61, 1.545, 3200);

        // 当日建仓:只亏买入价到现价这一小段。
        let new = today_pnl(true, buy, prev_close, cur, qty).unwrap();
        assert!((new - (-3.2)).abs() < 1e-9, "当日建仓应为 -3.2,实得 {new}");

        // 历史持仓:仍按昨收基准(-0.065/股 × 3200)。
        let held = today_pnl(false, buy, prev_close, cur, qty).unwrap();
        assert!(
            (held - (-208.0)).abs() < 1e-9,
            "历史持仓应为 -208,实得 {held}"
        );

        // 修复的意义:昨收基准把用户没经历过的 1.61→1.546 也算成了当日亏损。
        assert!(new > held);
    }

    #[test]
    fn diluted_folds_realized_into_cost() {
        // 成本 10000、市值 9000(浮亏 1000)、已做T赚了 1500。
        let (cost, value, realized) = (10_000.0, 9_000.0, 1_500.0);

        // 买入口径:浮亏就是浮亏,已实现另算。
        let (pnl, pct) = mode_pnl(CostMode::Buy, cost, value, realized);
        assert!((pnl + 1000.0).abs() < 1e-9);
        assert!((pct.unwrap() + 0.1).abs() < 1e-9);

        // 摊薄口径:成本降到 8500,盈亏变成该股总盈亏 500 = -1000 + 1500。
        let (pnl_d, pct_d) = mode_pnl(CostMode::Diluted, cost, value, realized);
        assert!((pnl_d - 500.0).abs() < 1e-9);
        assert!((pnl_d - (pnl + realized)).abs() < 1e-9);
        assert!((pct_d.unwrap() - 500.0 / 8500.0).abs() < 1e-9);
    }

    #[test]
    fn diluted_pct_none_when_cost_eaten_up() {
        // 已实现盈亏超过在场成本 → 摊薄成本 ≤0,收益率没有意义(东财此时也不给数)。
        let (pnl, pct) = mode_pnl(CostMode::Diluted, 1_000.0, 1_200.0, 1_000.0);
        assert!((pnl - 1200.0).abs() < 1e-9); // 盈亏本身仍成立:总共赚了 1200
        assert_eq!(pct, None);
        assert_eq!(
            mode_pnl(CostMode::Diluted, 1_000.0, 1_200.0, 1_500.0).1,
            None
        );
        // 零成本持仓(误录)在两种口径下都不给收益率,不做除零。
        assert_eq!(mode_pnl(CostMode::Buy, 0.0, 100.0, 0.0).1, None);
    }

    #[test]
    fn no_sells_means_modes_agree() {
        // 从没卖过时两种口径必须完全一致,否则默认切换会平白改变老用户看到的数字。
        for (cost, value) in [(1000.0, 1200.0), (5000.0, 3000.0)] {
            assert_eq!(
                mode_pnl(CostMode::Buy, cost, value, 0.0),
                mode_pnl(CostMode::Diluted, cost, value, 0.0)
            );
        }
    }

    #[test]
    fn basis_switches_on_position_date() {
        let today = today();
        // 误录未来日期时也按当日建仓处理:昨收基准对它更无意义。
        let future = "9999-12-31";
        assert!(future >= today.as_str());
        assert_eq!(today_basis(true, 10.0, 9.0), Some(10.0));
        assert_eq!(today_basis(false, 10.0, 9.0), Some(9.0));
    }

    #[test]
    fn invalid_basis_excluded_from_today() {
        // 源未给出昨收(0)的历史持仓:整条排除,不能拿 0 当基准把现价全算成当日盈利。
        assert_eq!(today_basis(false, 10.0, 0.0), None);
        assert_eq!(today_pnl(false, 10.0, 0.0, 11.0, 100), None);
        // 当日建仓但买入价非法(0)同样排除。
        assert_eq!(today_basis(true, 0.0, 9.0), None);
    }
}
