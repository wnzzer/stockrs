use anyhow::{anyhow, Result};

use crate::data::{resolve_input, source, Store};
use crate::indicator;

pub async fn run(code: String, period: usize) -> Result<()> {
    let (code, market) = resolve_input(&code).ok_or_else(|| anyhow!("无法识别的代码 {}", code))?;
    let store = Store::open_default()?;
    let mut klines = store.get_klines(&code, None, None)?;
    if klines.is_empty() {
        // 指数从不 data add(且与同码股票在 klines 表按 code 冲突),普通股也可能未跟踪:
        // 直连行情源在线取数,仅用于本次计算,不落库(避免与同码标的互相覆盖)。
        let (_, ks, _) = source::fetch_klines(&code, market, "0", "0")
            .await
            .map_err(|e| anyhow!("{} 无本地数据,在线获取也失败:{}", code, e))?;
        klines = ks;
    }
    if klines.is_empty() {
        return Err(anyhow!("{} 无数据", code));
    }

    let close: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let high: Vec<f64> = klines.iter().map(|k| k.high).collect();
    let low: Vec<f64> = klines.iter().map(|k| k.low).collect();
    let last = klines.len() - 1;
    let latest = &klines[last];

    let ma = |p: usize| fmt(indicator::sma(&close, p)[last]);
    let ema = |p: usize| fmt(indicator::ema(&close, p)[last]);

    println!("{} 最新技术指标（{}）", code, latest.date);
    println!("收盘价：{:.2}", latest.close);
    println!();
    println!("MA{:<3} : {}", period, ma(period));
    println!(
        "MA5   : {}   MA10  : {}   MA20  : {}",
        ma(5),
        ma(10),
        ma(20)
    );
    println!("EMA12 : {}   EMA26 : {}", ema(12), ema(26));
    println!(
        "RSI{:<2}: {}",
        period,
        fmt(indicator::rsi(&close, period)[last])
    );

    let m = indicator::macd(&close, 12, 26, 9);
    println!(
        "MACD  : DIF {}  DEA {}  MACD {}",
        fmt(m.dif[last]),
        fmt(m.dea[last]),
        fmt(m.macd[last])
    );

    let k = indicator::kdj(&high, &low, &close, 9);
    println!(
        "KDJ   : K {}  D {}  J {}",
        fmt(k.k[last]),
        fmt(k.d[last]),
        fmt(k.j[last])
    );

    let b = indicator::boll(&close, period, 2.0);
    println!(
        "BOLL  : 上 {}  中 {}  下 {}",
        fmt(b.upper[last]),
        fmt(b.mid[last]),
        fmt(b.lower[last])
    );

    // 基本面(最新一条,若已 data add/update 拉过)
    if let Some(f) = store.get_fundamentals(&code, None, None)?.last() {
        let fv = |v: Option<f64>| match v {
            Some(x) => format!("{:.2}", x),
            None => "--".to_string(),
        };
        let mv = match f.total_mv {
            Some(v) => format!("{:.1}亿", v / 1e8),
            None => "--".to_string(),
        };
        println!();
        println!(
            "PE(TTM): {}   PB: {}   PS(TTM): {}   总市值: {}   (截至 {})",
            fv(f.pe_ttm),
            fv(f.pb_mrq),
            fv(f.ps_ttm),
            mv,
            f.date
        );
    }
    Ok(())
}

fn fmt(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{:.2}", v),
        None => "--".to_string(),
    }
}
