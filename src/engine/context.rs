use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::data::{fundamental, Fundamental, KLine};
use crate::indicator;

/// 一笔成交记录（回测内部）。
#[derive(Clone)]
pub struct TradeRec {
    pub date: String,
    pub action: String,
    pub price: f64,
    pub shares: i64,
    pub pnl_pct: Option<f64>,
}

pub struct Order {
    pub buy: bool,
    pub shares: i64,
}

pub struct Fees {
    pub buy_rate: f64,
    pub sell_rate: f64,
    pub stamp_rate: f64,
}

impl Default for Fees {
    fn default() -> Self {
        // 买入万三，卖出万三 + 千一印花税
        Fees {
            buy_rate: 0.0003,
            sell_rate: 0.0003,
            stamp_rate: 0.001,
        }
    }
}

pub struct Inner {
    pub klines: Vec<KLine>,
    pub closes: Vec<f64>,
    pub highs: Vec<f64>,
    pub lows: Vec<f64>,
    pub i: usize,
    pub cash: f64,
    pub position: i64,
    pub avg_cost: f64,
    pub pending: Option<Order>,
    pub trades: Vec<TradeRec>,
    pub equity: Vec<f64>,
    pub fees: Fees,
    /// 参数扫描注入的键值（供 ctx.param 读取）；普通回测为空。
    pub params: HashMap<String, f64>,
    /// 基本面按 bar 对齐后的序列(与 klines 等长,无数据处 NaN)。
    pub pe: Vec<f64>,
    pub pb: Vec<f64>,
    pub ps: Vec<f64>,
    pub mv: Vec<f64>,
    /// 指标缓存:key(如 "sma:5")→ 全序列(NaN 代 None),避免每 bar 每次调用重算。
    pub cache: HashMap<String, Vec<f64>>,
    /// 策略自定义状态(跨 bar 持久),供 ctx.set/get/has 读写,如记录建仓价做止盈止损。
    pub state: HashMap<String, f64>,
}

/// 传给 Rhai 策略的上下文句柄，内部共享可变状态。
#[derive(Clone)]
pub struct Ctx(pub Arc<Mutex<Inner>>);

impl Ctx {
    pub fn new(klines: Vec<KLine>, capital: f64, funda: &[Fundamental]) -> Ctx {
        Ctx::new_with_params(klines, capital, HashMap::new(), funda)
    }

    pub fn new_with_params(
        klines: Vec<KLine>,
        capital: f64,
        params: HashMap<String, f64>,
        funda: &[Fundamental],
    ) -> Ctx {
        let closes = klines.iter().map(|k| k.close).collect();
        let highs = klines.iter().map(|k| k.high).collect();
        let lows = klines.iter().map(|k| k.low).collect();
        let dates: Vec<String> = klines.iter().map(|k| k.date.clone()).collect();
        let a = fundamental::align(&dates, funda);
        Ctx(Arc::new(Mutex::new(Inner {
            klines,
            closes,
            highs,
            lows,
            i: 0,
            cash: capital,
            position: 0,
            avg_cost: 0.0,
            pending: None,
            trades: Vec::new(),
            equity: Vec::new(),
            fees: Fees::default(),
            params,
            pe: a.pe,
            pb: a.pb,
            ps: a.ps,
            mv: a.mv,
            cache: HashMap::new(),
            state: HashMap::new(),
        })))
    }

    /// 回测结束后取资金曲线（供基准对齐）。
    pub fn equity_curve(&self) -> Vec<f64> {
        self.0.lock().unwrap().equity.clone()
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let mut g = self.0.lock().unwrap();
        f(&mut g)
    }

    // ---- 行情字段 ----
    pub fn open(&mut self) -> f64 {
        self.with(|s| s.klines[s.i].open)
    }
    pub fn high(&mut self) -> f64 {
        self.with(|s| s.klines[s.i].high)
    }
    pub fn low(&mut self) -> f64 {
        self.with(|s| s.klines[s.i].low)
    }
    pub fn close(&mut self) -> f64 {
        self.with(|s| s.klines[s.i].close)
    }
    pub fn volume(&mut self) -> f64 {
        self.with(|s| s.klines[s.i].volume)
    }
    pub fn date(&mut self) -> String {
        self.with(|s| s.klines[s.i].date.clone())
    }

    // ---- 历史数据 ----
    pub fn close_at(&mut self, n: i64) -> f64 {
        self.with(|s| {
            let idx = s.i as i64 - n;
            if idx < 0 {
                f64::NAN
            } else {
                s.closes[idx as usize]
            }
        })
    }

    // ---- 技术指标（截至当前 bar，全序列缓存，避免每 bar 重算+分配）----
    pub fn sma(&mut self, period: i64) -> f64 {
        self.sma_at(period, 0)
    }
    pub fn sma_at(&mut self, period: i64, n: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.with(|s| {
            indi_at(s, n, format!("sma:{}", period), move |s| {
                indicator::sma(&s.closes, p).into_iter().map(opt).collect()
            })
        })
    }
    pub fn ema(&mut self, period: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.with(|s| {
            indi_at(s, 0, format!("ema:{}", period), move |s| {
                indicator::ema(&s.closes, p).into_iter().map(opt).collect()
            })
        })
    }
    pub fn rsi(&mut self, period: i64) -> f64 {
        if period <= 0 {
            return f64::NAN;
        }
        let p = period as usize;
        self.with(|s| {
            indi_at(s, 0, format!("rsi:{}", period), move |s| {
                indicator::rsi(&s.closes, p).into_iter().map(opt).collect()
            })
        })
    }
    pub fn macd(&mut self, fast: i64, slow: i64, signal: i64) -> rhai::Array {
        let (f, sl, sg) = (fast as usize, slow as usize, signal as usize);
        let tag = format!("{}:{}:{}", fast, slow, signal);
        self.with(|s| {
            let dif = indi_at(s, 0, format!("macd.dif:{}", tag), move |s| {
                indicator::macd(&s.closes, f, sl, sg)
                    .dif
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let dea = indi_at(s, 0, format!("macd.dea:{}", tag), move |s| {
                indicator::macd(&s.closes, f, sl, sg)
                    .dea
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let mac = indi_at(s, 0, format!("macd.macd:{}", tag), move |s| {
                indicator::macd(&s.closes, f, sl, sg)
                    .macd
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            arr3(dif, dea, mac)
        })
    }
    pub fn kdj(&mut self, period: i64) -> rhai::Array {
        let p = period as usize;
        self.with(|s| {
            let k = indi_at(s, 0, format!("kdj.k:{}", period), move |s| {
                indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                    .k
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let d = indi_at(s, 0, format!("kdj.d:{}", period), move |s| {
                indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                    .d
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let j = indi_at(s, 0, format!("kdj.j:{}", period), move |s| {
                indicator::kdj(&s.highs, &s.lows, &s.closes, p)
                    .j
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            arr3(k, d, j)
        })
    }
    pub fn boll(&mut self, period: i64, mult: f64) -> rhai::Array {
        let p = period as usize;
        let tag = format!("{}:{}", period, mult);
        self.with(|s| {
            let up = indi_at(s, 0, format!("boll.u:{}", tag), move |s| {
                indicator::boll(&s.closes, p, mult)
                    .upper
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let mid = indi_at(s, 0, format!("boll.m:{}", tag), move |s| {
                indicator::boll(&s.closes, p, mult)
                    .mid
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            let low = indi_at(s, 0, format!("boll.l:{}", tag), move |s| {
                indicator::boll(&s.closes, p, mult)
                    .lower
                    .into_iter()
                    .map(opt)
                    .collect()
            });
            arr3(up, mid, low)
        })
    }

    // ---- 账户 ----
    pub fn position(&mut self) -> i64 {
        self.with(|s| s.position)
    }
    pub fn cash(&mut self) -> f64 {
        self.with(|s| s.cash)
    }
    pub fn total_value(&mut self) -> f64 {
        self.with(|s| s.cash + s.position as f64 * s.klines[s.i].close)
    }
    pub fn max_shares(&mut self) -> i64 {
        self.with(|s| {
            let price = s.klines[s.i].close;
            if price <= 0.0 {
                return 0;
            }
            let affordable = s.cash / (price * (1.0 + s.fees.buy_rate));
            (affordable as i64 / 100) * 100
        })
    }

    // ---- 参数注入（供参数扫描）----
    // rhai 按实参类型选择重载：ctx.param("fast", 5) 命中 i64 版返回 i64；
    // ctx.param("thresh", 30.0) 命中 f64 版返回 f64。缺省则回退到 default。
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

    // ---- 基本面（按 bar 对齐，无数据 NaN）----
    pub fn pe(&mut self) -> f64 {
        self.with(|s| s.pe.get(s.i).copied().unwrap_or(f64::NAN))
    }
    pub fn pb(&mut self) -> f64 {
        self.with(|s| s.pb.get(s.i).copied().unwrap_or(f64::NAN))
    }
    pub fn ps(&mut self) -> f64 {
        self.with(|s| s.ps.get(s.i).copied().unwrap_or(f64::NAN))
    }
    pub fn mktcap(&mut self) -> f64 {
        self.with(|s| s.mv.get(s.i).copied().unwrap_or(f64::NAN))
    }

    // ---- 策略状态（跨 bar 持久，挂在 ctx 上；如记录建仓价做止盈止损）----
    // set 按实参类型重载(整数存为 f64);get 按 default 类型选返回类型(整数四舍五入)。
    pub fn set_f(&mut self, key: String, val: f64) {
        self.with(|s| {
            s.state.insert(key, val);
        });
    }
    pub fn set_i(&mut self, key: String, val: i64) {
        self.with(|s| {
            s.state.insert(key, val as f64);
        });
    }
    pub fn get_f(&mut self, key: String, default: f64) -> f64 {
        self.with(|s| s.state.get(&key).copied().unwrap_or(default))
    }
    pub fn get_i(&mut self, key: String, default: i64) -> i64 {
        self.with(|s| {
            s.state
                .get(&key)
                .map(|v| v.round() as i64)
                .unwrap_or(default)
        })
    }
    pub fn has(&mut self, key: String) -> bool {
        self.with(|s| s.state.contains_key(&key))
    }

    // ---- 下单（次日开盘成交）----
    pub fn buy(&mut self, _price: f64, shares: i64) {
        self.with(|s| {
            if shares > 0 {
                s.pending = Some(Order { buy: true, shares });
            }
        });
    }
    pub fn sell(&mut self, _price: f64, shares: i64) {
        self.with(|s| {
            if shares > 0 {
                s.pending = Some(Order { buy: false, shares });
            }
        });
    }
}

/// 取指标 back 天前的值:首次计算整条并缓存,后续 O(1) 命中。越界/未算返回 NaN。
fn indi_at(s: &mut Inner, back: i64, key: String, compute: impl FnOnce(&Inner) -> Vec<f64>) -> f64 {
    let idx = s.i as i64 - back;
    if idx < 0 {
        return f64::NAN;
    }
    let uidx = idx as usize;
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

fn opt(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

fn arr3(a: f64, b: f64, c: f64) -> rhai::Array {
    vec![
        rhai::Dynamic::from_float(a),
        rhai::Dynamic::from_float(b),
        rhai::Dynamic::from_float(c),
    ]
}
