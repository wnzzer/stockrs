//! 多股票组合回测引擎。
//!
//! 与单标的引擎(context.rs / backtest.rs)不同,这里的策略 `on_bar(ctx)`
//! 每个**交易日**调用一次,一次能看到整个 universe,从而支持横截面选股(rank)
//! 与组合再平衡(order_target_pct)。
//!
//! 关键设计:
//! - master 日期轴 = 所有股票日期的**并集**并升序去重;某股停牌当日无 bar,
//!   按最近收盘 carry-forward 做市值估算(mark-to-market)。
//! - 每只股票的指标在其**自身连续序列**上计算并按 key 缓存,按 local index 取值。
//! - 撮合:订单在 bar i-1 的 on_bar 挂出,bar i 开盘成交,避免未来函数;
//!   同一 bar 内先撮合全部 SELL 再撮合 BUY,使卖出回笼现金可供买入。
//! - 确定性:stocks()/universe()/rank() 一律按 universe(Vec)顺序产出,
//!   rank 排除 NaN 动量并以 code 升序做 tie-break。

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rhai::{Array, Dynamic};

use super::context::{Fees, TradeRec};
use super::metrics::{self, Metrics};
use crate::data::{fundamental, Fundamental, KLine};
use crate::indicator;

/// 一只股票的行情输入。
#[derive(Clone)]
pub struct StockData {
    pub code: String,
    pub name: String,
    pub klines: Vec<KLine>,
    pub funda: Vec<Fundamental>,
}

/// 组合内一笔成交记录。
#[derive(Clone)]
pub struct PfTrade {
    pub date: String,
    pub code: String,
    pub action: String,
    pub price: f64,
    pub shares: i64,
    pub pnl_pct: Option<f64>,
}

/// 期末某只股票的持仓快照(供报告)。
pub struct Holding {
    pub code: String,
    pub name: String,
    pub shares: i64,
    pub avg_cost: f64,
    pub last_price: f64,
    pub value: f64,
    pub pnl_pct: f64,
}

pub struct PortfolioResult {
    pub metrics: Metrics,
    pub trades: Vec<PfTrade>,
    pub equity: Vec<f64>,
    pub dates: Vec<String>,
    pub holdings: Vec<Holding>,
    pub stock_count: usize,
}

struct Order {
    series: usize,
    buy: bool,
    shares: i64,
}

#[derive(Clone, Copy)]
struct Pos {
    shares: i64,
    avg_cost: f64,
}

/// 单只股票的连续行情序列 + 指标缓存。
struct Series {
    code: String,
    name: String,
    opens: Vec<f64>,
    highs: Vec<f64>,
    lows: Vec<f64>,
    closes: Vec<f64>,
    vols: Vec<f64>,
    /// 基本面按自身 bar 对齐(无数据 NaN)。
    pe: Vec<f64>,
    pb: Vec<f64>,
    ps: Vec<f64>,
    mv: Vec<f64>,
    idx_by_date: HashMap<String, usize>,
    /// 指标缓存:key(如 "sma:5")→ 对齐到自身序列的 Vec<f64>(不足处 NaN)。
    cache: HashMap<String, Vec<f64>>,
}

pub struct PfInner {
    dates: Vec<String>,
    series: Vec<Series>,
    code_to_idx: HashMap<String, usize>,
    i: usize,
    cash: f64,
    initial: f64,
    positions: HashMap<usize, Pos>,
    /// 每只股票最近一次已知收盘(停牌 carry-forward);持仓前为 0,不污染市值。
    last_price: Vec<f64>,
    pending: Vec<Order>,
    trades: Vec<PfTrade>,
    equity: Vec<f64>,
    fees: Fees,
    params: HashMap<String, f64>,
}

/// 传给 Rhai 组合策略的上下文句柄。
#[derive(Clone)]
pub struct PortfolioCtx(pub Arc<Mutex<PfInner>>);

fn opt(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

fn arr3(a: f64, b: f64, c: f64) -> Array {
    vec![
        Dynamic::from_float(a),
        Dynamic::from_float(b),
        Dynamic::from_float(c),
    ]
}

impl PfInner {
    fn local_idx(&self, si: usize) -> Option<usize> {
        self.series[si]
            .idx_by_date
            .get(&self.dates[self.i])
            .copied()
    }

    /// 取某股当前 bar 某价格字段;停牌/未知返回 NaN。
    fn field(&self, code: &str, which: u8) -> f64 {
        let si = match self.code_to_idx.get(code) {
            Some(&s) => s,
            None => return f64::NAN,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return f64::NAN,
        };
        let s = &self.series[si];
        match which {
            0 => s.opens[li],
            1 => s.highs[li],
            2 => s.lows[li],
            3 => s.closes[li],
            _ => s.vols[li],
        }
    }

    /// 当前 bar 某股基本面(0=pe 1=pb 2=ps 3=mv);停牌/未知/无数据 NaN。
    fn funda(&self, code: &str, which: u8) -> f64 {
        let si = match self.code_to_idx.get(code) {
            Some(&s) => s,
            None => return f64::NAN,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return f64::NAN,
        };
        let s = &self.series[si];
        let arr = match which {
            0 => &s.pe,
            1 => &s.pb,
            2 => &s.ps,
            _ => &s.mv,
        };
        arr.get(li).copied().unwrap_or(f64::NAN)
    }

    fn close_at(&self, code: &str, n: i64) -> f64 {
        let si = match self.code_to_idx.get(code) {
            Some(&s) => s,
            None => return f64::NAN,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return f64::NAN,
        };
        let idx = li as i64 - n;
        if idx < 0 {
            f64::NAN
        } else {
            self.series[si].closes[idx as usize]
        }
    }

    /// 计算(或命中缓存)某股指标序列,返回 back 天前的值。停牌/越界返回 NaN。
    fn indicator_at(
        &mut self,
        code: &str,
        back: i64,
        key: String,
        compute: impl FnOnce(&Series) -> Vec<f64>,
    ) -> f64 {
        let si = match self.code_to_idx.get(code).copied() {
            Some(s) => s,
            None => return f64::NAN,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return f64::NAN,
        };
        let idx = li as i64 - back;
        if idx < 0 {
            return f64::NAN;
        }
        let uidx = idx as usize;
        let s = &mut self.series[si];
        if !s.cache.contains_key(&key) {
            let v = compute(&*s);
            s.cache.insert(key.clone(), v);
        }
        s.cache
            .get(&key)
            .and_then(|v| v.get(uidx))
            .copied()
            .unwrap_or(f64::NAN)
    }

    fn sma_at(&mut self, code: &str, period: i64, n: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.indicator_at(code, n, format!("sma:{}", period), move |s| {
            indicator::sma(&s.closes, p).into_iter().map(opt).collect()
        })
    }

    fn ema_val(&mut self, code: &str, period: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.indicator_at(code, 0, format!("ema:{}", period), move |s| {
            indicator::ema(&s.closes, p).into_iter().map(opt).collect()
        })
    }

    fn rsi_val(&mut self, code: &str, period: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.indicator_at(code, 0, format!("rsi:{}", period), move |s| {
            indicator::rsi(&s.closes, p).into_iter().map(opt).collect()
        })
    }

    fn macd_arr(&mut self, code: &str, fast: i64, slow: i64, signal: i64) -> Array {
        let (f, sl, sg) = (fast as usize, slow as usize, signal as usize);
        let tag = format!("{}:{}:{}", fast, slow, signal);
        let dif = self.indicator_at(code, 0, format!("macd.dif:{}", tag), move |s| {
            indicator::macd(&s.closes, f, sl, sg)
                .dif
                .into_iter()
                .map(opt)
                .collect()
        });
        let dea = self.indicator_at(code, 0, format!("macd.dea:{}", tag), move |s| {
            indicator::macd(&s.closes, f, sl, sg)
                .dea
                .into_iter()
                .map(opt)
                .collect()
        });
        let mac = self.indicator_at(code, 0, format!("macd.macd:{}", tag), move |s| {
            indicator::macd(&s.closes, f, sl, sg)
                .macd
                .into_iter()
                .map(opt)
                .collect()
        });
        arr3(dif, dea, mac)
    }

    fn kdj_arr(&mut self, code: &str, period: i64) -> Array {
        let p = period as usize;
        let k = self.indicator_at(code, 0, format!("kdj.k:{}", period), move |s| {
            indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                .k
                .into_iter()
                .map(opt)
                .collect()
        });
        let d = self.indicator_at(code, 0, format!("kdj.d:{}", period), move |s| {
            indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                .d
                .into_iter()
                .map(opt)
                .collect()
        });
        let j = self.indicator_at(code, 0, format!("kdj.j:{}", period), move |s| {
            indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                .j
                .into_iter()
                .map(opt)
                .collect()
        });
        arr3(k, d, j)
    }

    fn boll_arr(&mut self, code: &str, period: i64, mult: f64) -> Array {
        let p = period as usize;
        let tag = format!("{}:{}", period, mult);
        let up = self.indicator_at(code, 0, format!("boll.u:{}", tag), move |s| {
            indicator::boll(&s.closes, p, mult)
                .upper
                .into_iter()
                .map(opt)
                .collect()
        });
        let mid = self.indicator_at(code, 0, format!("boll.m:{}", tag), move |s| {
            indicator::boll(&s.closes, p, mult)
                .mid
                .into_iter()
                .map(opt)
                .collect()
        });
        let low = self.indicator_at(code, 0, format!("boll.l:{}", tag), move |s| {
            indicator::boll(&s.closes, p, mult)
                .lower
                .into_iter()
                .map(opt)
                .collect()
        });
        arr3(up, mid, low)
    }

    /// 当前 bar 活跃(有 bar)的代码,按 universe 顺序。
    fn active_codes(&self) -> Array {
        let d = &self.dates[self.i];
        self.series
            .iter()
            .filter(|s| s.idx_by_date.contains_key(d))
            .map(|s| Dynamic::from(s.code.clone()))
            .collect()
    }

    fn all_codes(&self) -> Array {
        self.series
            .iter()
            .map(|s| Dynamic::from(s.code.clone()))
            .collect()
    }

    /// 活跃股按 lookback 日动量降序;排除数据不足(NaN)股;相等按 code 升序 tie-break。
    fn rank(&self, lookback: i64) -> Array {
        let d = &self.dates[self.i];
        let mut scored: Vec<(f64, String)> = Vec::new();
        for s in &self.series {
            if let Some(&li) = s.idx_by_date.get(d) {
                let idx = li as i64 - lookback;
                if idx < 0 {
                    continue;
                }
                let past = s.closes[idx as usize];
                let cur = s.closes[li];
                if past > 0.0 && cur.is_finite() {
                    scored.push((cur / past - 1.0, s.code.clone()));
                }
            }
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().map(|(_, c)| Dynamic::from(c)).collect()
    }

    fn total_value(&self) -> f64 {
        let mut tv = self.cash;
        for (&si, pos) in self.positions.iter() {
            tv += pos.shares as f64 * self.last_price[si];
        }
        tv
    }

    fn position_shares(&self, code: &str) -> i64 {
        self.code_to_idx
            .get(code)
            .and_then(|si| self.positions.get(si))
            .map(|p| p.shares)
            .unwrap_or(0)
    }

    fn avg_cost(&self, code: &str) -> f64 {
        self.code_to_idx
            .get(code)
            .and_then(|si| self.positions.get(si))
            .map(|p| p.avg_cost)
            .unwrap_or(0.0)
    }

    fn max_shares(&self, code: &str) -> i64 {
        let si = match self.code_to_idx.get(code) {
            Some(&s) => s,
            None => return 0,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return 0,
        };
        let price = self.series[si].closes[li];
        if price <= 0.0 {
            return 0;
        }
        let affordable = self.cash / (price * (1.0 + self.fees.buy_rate));
        (affordable as i64 / 100) * 100
    }

    fn queue(&mut self, code: &str, buy: bool, shares: i64) {
        if shares <= 0 {
            return;
        }
        if let Some(&si) = self.code_to_idx.get(code) {
            self.pending.push(Order {
                series: si,
                buy,
                shares,
            });
        }
    }

    /// 再平衡到目标权重:目标市值 = pct × 当前总资产,整手,与当前持仓 diff 后挂单。
    fn order_target(&mut self, code: &str, pct: f64) {
        let si = match self.code_to_idx.get(code).copied() {
            Some(s) => s,
            None => return,
        };
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return,
        };
        let price = self.series[si].closes[li];
        if price <= 0.0 {
            return;
        }
        let target_value = pct * self.total_value();
        let target_shares = ((target_value / price) as i64 / 100) * 100;
        let cur = self.positions.get(&si).map(|p| p.shares).unwrap_or(0);
        let diff = target_shares - cur;
        if diff > 0 {
            self.pending.push(Order {
                series: si,
                buy: true,
                shares: diff,
            });
        } else if diff < 0 {
            self.pending.push(Order {
                series: si,
                buy: false,
                shares: -diff,
            });
        }
    }

    /// 按当前 bar 开盘价撮合一笔订单。停牌(无 bar)则丢弃。
    fn fill(&mut self, order: Order) {
        let si = order.series;
        let li = match self.local_idx(si) {
            Some(l) => l,
            None => return,
        };
        let price = self.series[si].opens[li];
        if price <= 0.0 {
            return;
        }
        let date = self.dates[self.i].clone();
        let code = self.series[si].code.clone();

        if order.buy {
            let unit_cost = price * (1.0 + self.fees.buy_rate);
            let affordable = ((self.cash / unit_cost) as i64 / 100) * 100;
            let shares = order.shares.min(affordable);
            if shares <= 0 {
                return;
            }
            let cost = shares as f64 * price;
            let fee = cost * self.fees.buy_rate;
            {
                let pos = self.positions.entry(si).or_insert(Pos {
                    shares: 0,
                    avg_cost: 0.0,
                });
                let prev_cost = pos.avg_cost * pos.shares as f64;
                pos.shares += shares;
                // 成本口径不含买入手续费,与单标的引擎一致。
                pos.avg_cost = (prev_cost + cost) / pos.shares as f64;
            }
            self.cash -= cost + fee;
            self.trades.push(PfTrade {
                date,
                code,
                action: "BUY".to_string(),
                price,
                shares,
                pnl_pct: None,
            });
        } else {
            let held = self.positions.get(&si).map(|p| p.shares).unwrap_or(0);
            let shares = order.shares.min(held);
            if shares <= 0 {
                return;
            }
            let proceeds = shares as f64 * price;
            let fee = proceeds * (self.fees.sell_rate + self.fees.stamp_rate);
            let avg_cost = self.positions.get(&si).map(|p| p.avg_cost).unwrap_or(0.0);
            let pnl_pct = if avg_cost > 0.0 {
                Some((price - avg_cost) / avg_cost)
            } else {
                None
            };
            self.cash += proceeds - fee;
            if let Some(pos) = self.positions.get_mut(&si) {
                pos.shares -= shares;
                if pos.shares == 0 {
                    self.positions.remove(&si);
                }
            }
            self.trades.push(PfTrade {
                date,
                code,
                action: "SELL".to_string(),
                price,
                shares,
                pnl_pct,
            });
        }
    }
}

impl PortfolioCtx {
    pub fn new(stocks: Vec<StockData>, capital: f64, params: HashMap<String, f64>) -> PortfolioCtx {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for sd in &stocks {
            for k in &sd.klines {
                set.insert(k.date.clone());
            }
        }
        let dates: Vec<String> = set.into_iter().collect(); // BTreeSet<String> 即按日期升序

        let mut series = Vec::with_capacity(stocks.len());
        let mut code_to_idx = HashMap::new();
        for (i, sd) in stocks.into_iter().enumerate() {
            code_to_idx.insert(sd.code.clone(), i);
            let n = sd.klines.len();
            let mut idx_by_date = HashMap::with_capacity(n);
            let mut opens = Vec::with_capacity(n);
            let mut highs = Vec::with_capacity(n);
            let mut lows = Vec::with_capacity(n);
            let mut closes = Vec::with_capacity(n);
            let mut vols = Vec::with_capacity(n);
            let mut sdates = Vec::with_capacity(n);
            for (li, k) in sd.klines.iter().enumerate() {
                idx_by_date.insert(k.date.clone(), li);
                opens.push(k.open);
                highs.push(k.high);
                lows.push(k.low);
                closes.push(k.close);
                vols.push(k.volume);
                sdates.push(k.date.clone());
            }
            let a = fundamental::align(&sdates, &sd.funda);
            series.push(Series {
                code: sd.code,
                name: sd.name,
                opens,
                highs,
                lows,
                closes,
                vols,
                pe: a.pe,
                pb: a.pb,
                ps: a.ps,
                mv: a.mv,
                idx_by_date,
                cache: HashMap::new(),
            });
        }

        let ns = series.len();
        PortfolioCtx(Arc::new(Mutex::new(PfInner {
            dates,
            series,
            code_to_idx,
            i: 0,
            cash: capital,
            initial: capital,
            positions: HashMap::new(),
            last_price: vec![0.0; ns],
            pending: Vec::new(),
            trades: Vec::new(),
            equity: Vec::new(),
            fees: Fees::default(),
            params,
        })))
    }

    fn with<T>(&self, f: impl FnOnce(&mut PfInner) -> T) -> T {
        let mut g = self.0.lock().unwrap();
        f(&mut g)
    }

    pub fn dates(&self) -> Vec<String> {
        self.0.lock().unwrap().dates.clone()
    }

    // ---- universe / 选股 ----
    pub fn date(&mut self) -> String {
        self.with(|s| s.dates[s.i].clone())
    }
    pub fn stocks(&mut self) -> Array {
        self.with(|s| s.active_codes())
    }
    pub fn universe(&mut self) -> Array {
        self.with(|s| s.all_codes())
    }
    pub fn rank(&mut self, lookback: i64) -> Array {
        self.with(|s| s.rank(lookback))
    }

    // ---- 行情 ----
    pub fn open(&mut self, code: String) -> f64 {
        self.with(|s| s.field(&code, 0))
    }
    pub fn high(&mut self, code: String) -> f64 {
        self.with(|s| s.field(&code, 1))
    }
    pub fn low(&mut self, code: String) -> f64 {
        self.with(|s| s.field(&code, 2))
    }
    pub fn close(&mut self, code: String) -> f64 {
        self.with(|s| s.field(&code, 3))
    }
    pub fn volume(&mut self, code: String) -> f64 {
        self.with(|s| s.field(&code, 4))
    }
    pub fn close_at(&mut self, code: String, n: i64) -> f64 {
        self.with(|s| s.close_at(&code, n))
    }

    // ---- 指标 ----
    pub fn sma(&mut self, code: String, period: i64) -> f64 {
        self.with(|s| s.sma_at(&code, period, 0))
    }
    pub fn sma_at(&mut self, code: String, period: i64, n: i64) -> f64 {
        self.with(|s| s.sma_at(&code, period, n))
    }
    pub fn ema(&mut self, code: String, period: i64) -> f64 {
        self.with(|s| s.ema_val(&code, period))
    }
    pub fn rsi(&mut self, code: String, period: i64) -> f64 {
        self.with(|s| s.rsi_val(&code, period))
    }
    pub fn macd(&mut self, code: String, fast: i64, slow: i64, signal: i64) -> Array {
        self.with(|s| s.macd_arr(&code, fast, slow, signal))
    }
    pub fn kdj(&mut self, code: String, period: i64) -> Array {
        self.with(|s| s.kdj_arr(&code, period))
    }
    pub fn boll(&mut self, code: String, period: i64, mult: f64) -> Array {
        self.with(|s| s.boll_arr(&code, period, mult))
    }

    // ---- 账户 ----
    pub fn position(&mut self, code: String) -> i64 {
        self.with(|s| s.position_shares(&code))
    }
    pub fn avg_cost(&mut self, code: String) -> f64 {
        self.with(|s| s.avg_cost(&code))
    }
    pub fn cash(&mut self) -> f64 {
        self.with(|s| s.cash)
    }
    pub fn total_value(&mut self) -> f64 {
        self.with(|s| s.total_value())
    }
    pub fn max_shares(&mut self, code: String) -> i64 {
        self.with(|s| s.max_shares(&code))
    }

    // ---- 基本面（按 bar 对齐，无数据 NaN）----
    pub fn pe(&mut self, code: String) -> f64 {
        self.with(|s| s.funda(&code, 0))
    }
    pub fn pb(&mut self, code: String) -> f64 {
        self.with(|s| s.funda(&code, 1))
    }
    pub fn ps(&mut self, code: String) -> f64 {
        self.with(|s| s.funda(&code, 2))
    }
    pub fn mktcap(&mut self, code: String) -> f64 {
        self.with(|s| s.funda(&code, 3))
    }

    // ---- 下单(次日开盘成交)----
    pub fn buy(&mut self, code: String, shares: i64) {
        self.with(|s| s.queue(&code, true, shares));
    }
    pub fn sell(&mut self, code: String, shares: i64) {
        self.with(|s| s.queue(&code, false, shares));
    }
    // pct 必须为浮点(如 0.5);额外注册 i64 版兜底脚本误写整数的情况。
    pub fn order_target_pct_f(&mut self, code: String, pct: f64) {
        self.with(|s| s.order_target(&code, pct));
    }
    pub fn order_target_pct_i(&mut self, code: String, pct: i64) {
        self.with(|s| s.order_target(&code, pct as f64));
    }

    // ---- 参数注入 ----
    pub fn param_i(&mut self, name: String, default: i64) -> i64 {
        self.with(|s| {
            s.params
                .get(&name)
                .map(|v| v.round() as i64)
                .unwrap_or(default)
        })
    }
    pub fn param_f(&mut self, name: String, default: f64) -> f64 {
        self.with(|s| s.params.get(&name).copied().unwrap_or(default))
    }
}

/// 驱动组合回测主循环。每个交易日:先撮合上一 bar 挂出的订单(先卖后买),
/// 更新市值,再调用 on_bar。
pub fn run<F>(ctx: &PortfolioCtx, mut on_bar: F) -> Result<PortfolioResult>
where
    F: FnMut() -> Result<()>,
{
    let n = ctx.0.lock().unwrap().dates.len();
    let initial = ctx.0.lock().unwrap().initial;

    for i in 0..n {
        {
            let mut s = ctx.0.lock().unwrap();
            s.i = i;
            // 先卖后买:false(sell) 排在 true(buy) 前,卖出回笼现金可供买入。
            let mut orders = std::mem::take(&mut s.pending);
            orders.sort_by_key(|o| o.buy);
            for o in orders {
                s.fill(o);
            }
            // 更新活跃股最近收盘价(停牌股沿用旧值)。
            for si in 0..s.series.len() {
                if let Some(li) = s.series[si].idx_by_date.get(&s.dates[i]).copied() {
                    let px = s.series[si].closes[li];
                    s.last_price[si] = px;
                }
            }
            let eq = s.total_value();
            s.equity.push(eq);
        }
        on_bar()?;
    }

    let s = ctx.0.lock().unwrap();
    let trs: Vec<TradeRec> = s
        .trades
        .iter()
        .map(|t| TradeRec {
            date: t.date.clone(),
            action: t.action.clone(),
            price: t.price,
            shares: t.shares,
            pnl_pct: t.pnl_pct,
        })
        .collect();
    let metrics = metrics::compute(initial, &s.equity, &trs);

    let mut holdings: Vec<Holding> = s
        .positions
        .iter()
        .map(|(&si, pos)| {
            let last = s.last_price[si];
            Holding {
                code: s.series[si].code.clone(),
                name: s.series[si].name.clone(),
                shares: pos.shares,
                avg_cost: pos.avg_cost,
                last_price: last,
                value: pos.shares as f64 * last,
                pnl_pct: if pos.avg_cost > 0.0 {
                    (last - pos.avg_cost) / pos.avg_cost
                } else {
                    0.0
                },
            }
        })
        .collect();
    holdings.sort_by(|a, b| a.code.cmp(&b.code));

    Ok(PortfolioResult {
        metrics,
        trades: s.trades.clone(),
        equity: s.equity.clone(),
        dates: s.dates.clone(),
        holdings,
        stock_count: s.series.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(date: &str, o: f64, c: f64) -> KLine {
        KLine {
            code: "T".into(),
            date: date.into(),
            open: o,
            high: c.max(o),
            low: c.min(o),
            close: c,
            volume: 0.0,
            amount: 0.0,
            turnover: None,
        }
    }

    fn stock(code: &str, bars: &[(&str, f64, f64)]) -> StockData {
        StockData {
            code: code.into(),
            name: code.into(),
            klines: bars.iter().map(|(d, o, c)| k(d, *o, *c)).collect(),
            funda: Vec::new(),
        }
    }

    #[test]
    fn buy_fills_next_open_across_two_stocks() {
        let a = stock(
            "A",
            &[("d1", 10.0, 10.0), ("d2", 10.0, 10.0), ("d3", 12.0, 12.0)],
        );
        let b = stock(
            "B",
            &[("d1", 20.0, 20.0), ("d2", 20.0, 20.0), ("d3", 20.0, 20.0)],
        );
        let ctx = PortfolioCtx::new(vec![a, b], 100_000.0, HashMap::new());
        let c = ctx.clone();
        let mut bar = 0;
        let res = run(&ctx, move || {
            // d1 买 A 100 股 -> d2 开盘 10 成交;d2 卖 A -> d3 开盘 12 成交
            if bar == 0 {
                c.clone().buy("A".into(), 100);
            } else if bar == 1 {
                c.clone().sell("A".into(), 100);
            }
            bar += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(res.trades.len(), 2);
        assert_eq!(res.trades[0].action, "BUY");
        assert_eq!(res.trades[0].code, "A");
        assert_eq!(res.trades[1].action, "SELL");
        let pnl = res.trades[1].pnl_pct.unwrap();
        assert!((pnl - 0.2).abs() < 1e-6, "pnl={}", pnl);
        assert!(res.metrics.final_value > 100_000.0);
        assert_eq!(res.equity.len(), 3);
    }

    #[test]
    fn rank_orders_by_momentum_desc_and_excludes_nan() {
        // A 涨 20%,B 涨 10%,C 数据不足(仅 1 bar)。rank 应为 [A, B],排除 C。
        let a = stock("A", &[("d1", 10.0, 10.0), ("d2", 10.0, 12.0)]);
        let b = stock("B", &[("d1", 10.0, 10.0), ("d2", 10.0, 11.0)]);
        let c = stock("C", &[("d2", 5.0, 5.0)]);
        let ctx = PortfolioCtx::new(vec![a, b, c], 100_000.0, HashMap::new());
        let handle = ctx.clone();
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink2 = sink.clone();
        run(&ctx, move || {
            if handle.clone().date() == "d2" {
                let r = handle.clone().rank(1);
                let codes: Vec<String> = r.into_iter().map(|d| d.into_string().unwrap()).collect();
                *sink2.lock().unwrap() = codes;
            }
            Ok(())
        })
        .unwrap();
        let captured = sink.lock().unwrap().clone();
        assert_eq!(captured, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn order_target_pct_rebalances_to_weight() {
        // 单股 universe,d1 order_target_pct(0.5) -> d2 开盘 10 买入约半仓。
        let a = stock(
            "A",
            &[("d1", 10.0, 10.0), ("d2", 10.0, 10.0), ("d3", 10.0, 10.0)],
        );
        let ctx = PortfolioCtx::new(vec![a], 100_000.0, HashMap::new());
        let c = ctx.clone();
        let mut bar = 0;
        let res = run(&ctx, move || {
            if bar == 0 {
                c.clone().order_target_pct_f("A".into(), 0.5);
            }
            bar += 1;
            Ok(())
        })
        .unwrap();
        // 目标市值 50000,价 10 -> 5000 股;成交后持仓 5000 股。
        let h = &res.holdings;
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].shares, 5000);
    }
}
