//! 雪球数据源。
//!
//! 三个逆向出来的坑,改动前务必先读:
//!
//! 1. **必须自己声明 gzip 并解压**。不带 `Accept-Encoding: gzip` 时雪球返回
//!    HTTP 200 + **空 body**(而不是报错码),下游只会看到一句莫名的 JSON EOF。
//!    项目的 reqwest 关掉了默认 features(没有 gzip,省 async-compression),
//!    所以这里显式声明并手工 gunzip;flate2 本来就在依赖树里(self_update 带的)。
//!
//! 2. **UA 不能是 `curl/*`**:那个前缀直接 403 "IP Blacklisted"。项目共享客户端的
//!    "Mozilla/5.0 (stockrs)" 实测可用,所以直接复用 `http_client()`,不另造客户端。
//!
//! 3. **K线需要登录 cookie**:匿名调 kline.json 返回 `error_code: "400016"`
//!    ("请刷新页面或者重新登录帐号")。token 取浏览器 cookie 里 `xq_a_token` 的值,
//!    经环境变量 `XUEQIU_TOKEN` 提供;缺失时返回 Err,由 `fetch_klines` 的
//!    故障切换转下一个源。实时行情(quotec)不需要 token。
//!
//! 能力边界:实时行情覆盖沪深 + 港股(项目 `Market` 目前只有这三个;美股要等
//! `Market` 支持后再接)。quotec 不返回股票名称与 PE/PB —— 名称留空由
//! `cli::data::resolve_name` 兜底,PE/PB 留 None 与新浪同策(CLI 回退本地基本面并打 *)。
//! 限流:quotec 天生批量(N 只 1 个请求),批量更新的节流在 `cli::data` 那层已有。

use std::env;
use std::io::Read;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use serde_json::Value;

use super::models::{KLine, Market, Period, Quote};
use super::source::{http_client, KlineSource, QuoteSource};
use crate::utils::date;

const QUOTEC_URL: &str = "https://stock.xueqiu.com/v5/stock/realtime/quotec.json";
const KLINE_URL: &str = "https://stock.xueqiu.com/v5/stock/chart/kline.json";

/// 登录 token 的环境变量名(值为浏览器 cookie 里 xq_a_token 的内容)。
pub const TOKEN_ENV: &str = "XUEQIU_TOKEN";

/// beg 为 "0"(不限)时的历史起点。雪球 begin 需要一个具体时间戳,给 0 会被拒。
const EPOCH_FLOOR: &str = "1990-01-01";

pub struct Xueqiu;

/// 项目代码 → 雪球 symbol:沪深带市场前缀,港股就是补零后的 5 位代码。
fn symbol(code: &str, market: Market) -> String {
    match market {
        Market::SH => format!("SH{}", code),
        Market::SZ => format!("SZ{}", code),
        Market::HK => code.to_string(),
    }
}

/// 雪球 symbol → 项目代码。`fetch_quotes` 是按请求代码索引结果的,
/// 这里还原不干净会让整个源被当成"没有这只票"而静默跳过。
fn code_from_symbol(symbol: &str) -> &str {
    symbol
        .strip_prefix("SH")
        .or_else(|| symbol.strip_prefix("SZ"))
        .unwrap_or(symbol)
}

fn token() -> Option<String> {
    env::var(TOKEN_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// gunzip;万一雪球直接返回明文就按原文处理(不为一个 header 赌上整条链路)。
fn decode_body(raw: &[u8]) -> Result<String> {
    let mut out = String::new();
    if GzDecoder::new(raw).read_to_string(&mut out).is_ok() {
        return Ok(out);
    }
    String::from_utf8(raw.to_vec()).context("雪球响应既不是 gzip 也不是合法 UTF-8")
}

/// 雪球把业务错误塞在 200 响应体里,且 error_code 有时是数字 0、有时是字符串
/// "400016",两种都要认。
fn check_error(json: &Value) -> Result<()> {
    let failed = match json.get("error_code") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        Some(Value::String(s)) => !s.is_empty() && s != "0",
        _ => false,
    };
    if !failed {
        return Ok(());
    }
    let desc = json
        .get("error_description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("未知错误");
    let code = json
        .get("error_code")
        .map(|c| c.to_string())
        .unwrap_or_default();
    // 400016 = 未登录/登录过期,单独给出可操作的提示。
    if code.contains("400016") {
        bail!(
            "雪球要求登录({}):请将浏览器 cookie 中 xq_a_token 的值设入环境变量 {}",
            desc,
            TOKEN_ENV
        );
    }
    bail!("雪球接口报错 {}:{}", code, desc)
}

async fn get_json(url: &str, query: &[(&str, &str)], token: Option<&str>) -> Result<Value> {
    let mut req = http_client()?
        .get(url)
        .query(query)
        // 见模块文档坑 1:不声明 gzip 会拿到 200 + 空 body。
        .header(reqwest::header::ACCEPT_ENCODING, "gzip");
    if let Some(t) = token {
        // xq_a_token 与 xqat 是同一个值的两个名字,一起带上更稳。
        req = req.header(reqwest::header::COOKIE, format!("xq_a_token={t}; xqat={t}"));
    }
    let raw = req
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await
        .context("读取雪球响应失败")?;
    if raw.is_empty() {
        bail!("雪球返回空响应(通常是未声明 gzip 或被限流)");
    }
    let body = decode_body(&raw)?;
    let json: Value = serde_json::from_str(&body).context("解析雪球响应失败")?;
    check_error(&json)?;
    Ok(json)
}

/// 解析一条 quotec 记录。停牌/退市时 current 可能为 null,这类整条丢弃
/// (返回 None),让故障切换去下一个源补,而不是塞一个 0 价进组合。
fn parse_quote(d: &Value) -> Option<Quote> {
    let num = |k: &str| d.get(k).and_then(Value::as_f64);
    let symbol = d.get("symbol").and_then(Value::as_str)?;
    let price = num("current")?;
    Some(Quote {
        code: code_from_symbol(symbol).to_string(),
        // quotec 不返回名称;留空,由调用方(cli::data::resolve_name)兜底。
        name: String::new(),
        price,
        change: num("chg").unwrap_or(0.0),
        // percent 是百分数(4.3 表示 +4.3%),项目内部统一存小数。
        change_pct: num("percent").unwrap_or(0.0) / 100.0,
        open: num("open").unwrap_or(0.0),
        high: num("high").unwrap_or(0.0),
        low: num("low").unwrap_or(0.0),
        prev_close: num("last_close").unwrap_or(0.0),
        volume: num("volume").unwrap_or(0.0) / 100.0, // 股 → 手(与新浪同口径)
        amount: num("amount").unwrap_or(0.0),
        turnover: num("turnover_rate"),
        // quotec 不含 PE/PB(详情接口 quote.json 需 token)。同新浪留 None,
        // CLI 会回退本地基本面并打 * 标注。
        pe: None,
        pb: None,
    })
}

#[async_trait]
impl QuoteSource for Xueqiu {
    fn name(&self) -> &'static str {
        "xueqiu"
    }

    async fn quote(&self, code: &str, market: Market) -> Result<Quote> {
        self.quotes(&[(code.to_string(), market)])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("雪球行情返回空,代码 {} 可能不存在", code))
    }

    /// quotec 批量行情:symbol 逗号分隔,一次拿多只(A股 + 港股)。无需 token。
    async fn quotes(&self, reqs: &[(String, Market)]) -> Result<Vec<Quote>> {
        if reqs.is_empty() {
            return Ok(Vec::new());
        }
        let symbols = reqs
            .iter()
            .map(|(c, m)| symbol(c, *m))
            .collect::<Vec<_>>()
            .join(",");
        let json = get_json(QUOTEC_URL, &[("symbol", symbols.as_str())], None).await?;
        let empty = Vec::new();
        let data = json.get("data").and_then(Value::as_array).unwrap_or(&empty);
        Ok(data.iter().filter_map(parse_quote).collect())
    }
}

/// "YYYYMMDD" → 北京时间当日 00:00 的 epoch 毫秒;"0"/非法 → None。
fn compact_to_ms(s: &str) -> Option<i64> {
    if s.len() != 8 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    date::to_epoch_ms(&format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]))
}

/// 按列名取值。雪球 kline 用 column 声明列顺序,照名字查比按下标猜稳。
fn column_index(columns: &[&str], name: &str) -> Option<usize> {
    columns.iter().position(|c| *c == name)
}

fn parse_klines(code: &str, period: Period, data: &Value) -> Vec<KLine> {
    let columns: Vec<&str> = data
        .get("column")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let (i_ts, i_open, i_high, i_low, i_close) = (
        column_index(&columns, "timestamp"),
        column_index(&columns, "open"),
        column_index(&columns, "high"),
        column_index(&columns, "low"),
        column_index(&columns, "close"),
    );
    let (i_vol, i_amt, i_turn) = (
        column_index(&columns, "volume"),
        column_index(&columns, "amount"),
        column_index(&columns, "turnoverrate"),
    );

    let empty = Vec::new();
    let items = data.get("item").and_then(Value::as_array).unwrap_or(&empty);
    let mut out = Vec::with_capacity(items.len());
    for row in items {
        let row = match row.as_array() {
            Some(r) => r,
            None => continue,
        };
        let cell = |i: Option<usize>| i.and_then(|i| row.get(i)).and_then(Value::as_f64);
        // timestamp 与收盘价缺失的行没有意义,跳过。
        let (ts, close) = match (cell(i_ts), cell(i_close)) {
            (Some(t), Some(c)) => (t as i64, c),
            _ => continue,
        };
        out.push(KLine {
            code: code.to_string(),
            date: date::from_epoch_ms(ts, period.is_intraday()),
            open: cell(i_open).unwrap_or(close),
            high: cell(i_high).unwrap_or(close),
            low: cell(i_low).unwrap_or(close),
            close,
            volume: cell(i_vol).unwrap_or(0.0) / 100.0, // 股 → 手
            amount: cell(i_amt).unwrap_or(0.0),
            turnover: cell(i_turn),
        });
    }
    out
}

#[async_trait]
impl KlineSource for Xueqiu {
    fn name(&self) -> &'static str {
        "xueqiu"
    }

    /// K线(前复权,type=before,与东财 fqt=1 同口径)。**需要 token**,见模块文档坑 3。
    async fn klines(
        &self,
        code: &str,
        market: Market,
        period: Period,
        beg: &str,
        end: &str,
    ) -> Result<(String, Vec<KLine>)> {
        let token = token().ok_or_else(|| {
            anyhow!(
                "雪球K线需要登录 cookie:请将浏览器 cookie 中 xq_a_token 的值设入环境变量 {}",
                TOKEN_ENV
            )
        })?;
        let sym = symbol(code, market);
        let begin = compact_to_ms(beg)
            .or_else(|| date::to_epoch_ms(EPOCH_FLOOR))
            .unwrap_or(0)
            .to_string();
        // end 缺省用"现在";传未来日期(如 20500101)雪球会自行截到最新一根。
        let end_ms = compact_to_ms(end).unwrap_or_else(date::now_ms).to_string();

        let json = get_json(
            KLINE_URL,
            &[
                ("symbol", sym.as_str()),
                ("begin", begin.as_str()),
                ("end", end_ms.as_str()),
                ("period", period.xueqiu_period()),
                ("type", "before"),
                ("indicator", "kline"),
            ],
            Some(&token),
        )
        .await?;

        let data = json
            .get("data")
            .filter(|d| !d.is_null())
            .ok_or_else(|| anyhow!("雪球K线返回空数据,代码 {} 可能不存在", code))?;
        // 雪球 kline 只回 symbol,没有中文名;留空由 cli::data 回退 resolve_name。
        Ok((String::new(), parse_klines(code, period, data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 quotec 响应片段(2026-07-30 抓取,已裁剪无关字段)。
    const QUOTEC_SAMPLE: &str = r#"{"data":[
      {"symbol":"SZ000858","current":78.4,"percent":4.3,"chg":3.23,"volume":42992286,
       "amount":3.338939101E9,"turnover_rate":1.11,"open":75.1,"last_close":75.17,
       "high":78.74,"low":75.01},
      {"symbol":"SH600519","current":1355.36,"percent":2.6,"chg":34.36,"volume":3460448,
       "amount":4.663368848E9,"turnover_rate":0.28,"open":1323.0,"last_close":1321.0,
       "high":1358.0,"low":1322.0},
      {"symbol":"00700","current":465.4,"percent":-0.21,"chg":-1.0,"volume":9789459,
       "amount":4576759323.7,"turnover_rate":0.11,"open":466.4,"last_close":466.4,
       "high":474.4,"low":462.8}
    ],"error_code":0,"error_description":null}"#;

    #[test]
    fn symbol_mapping_roundtrip() {
        assert_eq!(symbol("600519", Market::SH), "SH600519");
        assert_eq!(symbol("000858", Market::SZ), "SZ000858");
        assert_eq!(symbol("00700", Market::HK), "00700"); // 港股不带前缀
        for (code, market) in [
            ("600519", Market::SH),
            ("000858", Market::SZ),
            ("00700", Market::HK),
        ] {
            // 往返必须一致,否则 fetch_quotes 索引不到结果会静默跳过本源。
            assert_eq!(code_from_symbol(&symbol(code, market)), code);
        }
    }

    #[test]
    fn parse_quotec_sample() {
        let json: Value = serde_json::from_str(QUOTEC_SAMPLE).unwrap();
        check_error(&json).unwrap();
        let qs: Vec<Quote> = json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(parse_quote)
            .collect();
        assert_eq!(qs.len(), 3);

        let mao = qs.iter().find(|q| q.code == "600519").unwrap();
        assert_eq!(mao.price, 1355.36);
        assert_eq!(mao.prev_close, 1321.0);
        assert!((mao.change_pct - 0.026).abs() < 1e-9); // percent 4.3 → 0.043 口径
        assert_eq!(mao.volume, 3460448.0 / 100.0); // 股 → 手
        assert_eq!(mao.turnover, Some(0.28));
        // quotec 不提供名称与 PE/PB
        assert!(mao.name.is_empty());
        assert!(mao.pe.is_none() && mao.pb.is_none());

        // 港股代码原样保留(不能被 strip 掉任何前缀)
        assert!(qs.iter().any(|q| q.code == "00700"));
        // 跌幅为负时符号要保住
        let tx = qs.iter().find(|q| q.code == "00700").unwrap();
        assert!(tx.change < 0.0 && tx.change_pct < 0.0);
    }

    #[test]
    fn quote_without_price_is_dropped() {
        // 停牌/退市:current 为 null → 整条丢弃,不能塞 0 价进组合。
        let json: Value =
            serde_json::from_str(r#"{"symbol":"SH600519","current":null,"chg":0}"#).unwrap();
        assert!(parse_quote(&json).is_none());
    }

    #[test]
    fn business_error_in_200_body_is_detected() {
        // 未登录:error_code 是字符串,不是数字
        let e: Value = serde_json::from_str(
            r#"{"error_description":"遇到错误，请刷新页面或者重新登录帐号后再试","error_code":"400016"}"#,
        )
        .unwrap();
        let msg = check_error(&e).unwrap_err().to_string();
        assert!(msg.contains(TOKEN_ENV), "应提示如何配置 token,实得:{msg}");

        // 正常响应:error_code 为数字 0
        let ok: Value = serde_json::from_str(r#"{"data":[],"error_code":0}"#).unwrap();
        assert!(check_error(&ok).is_ok());
        // 其它业务错误码
        let other: Value =
            serde_json::from_str(r#"{"error_code":"1000","error_description":"x"}"#).unwrap();
        assert!(check_error(&other).is_err());
    }

    #[test]
    fn gzip_and_plaintext_both_decode() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let payload = r#"{"error_code":0}"#;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(payload.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(decode_body(&gz).unwrap(), payload);
        // 兜底路径:未压缩的明文也要能读
        assert_eq!(decode_body(payload.as_bytes()).unwrap(), payload);
    }

    #[test]
    fn kline_parsed_by_column_name() {
        // 列顺序故意打乱:必须按 column 名字定位,不能按下标猜。
        let data: Value = serde_json::from_str(
            r#"{"symbol":"SH600519",
                "column":["volume","timestamp","close","open","high","low","amount","turnoverrate"],
                "item":[[3460448,1735660800000,1400.5,1390.0,1410.0,1385.0,4.8E9,0.28]]}"#,
        )
        .unwrap();
        let ks = parse_klines("600519", Period::Day, &data);
        assert_eq!(ks.len(), 1);
        let k = &ks[0];
        assert_eq!(k.code, "600519");
        assert_eq!(k.open, 1390.0);
        assert_eq!(k.close, 1400.5);
        assert_eq!(k.high, 1410.0);
        assert_eq!(k.low, 1385.0);
        assert_eq!(k.volume, 3460448.0 / 100.0);
        assert_eq!(k.turnover, Some(0.28));
        // 日线只要日期,不带 HH:MM
        assert_eq!(k.date, date::from_epoch_ms(1735660800000, false));
        assert!(!k.date.contains(':'));

        // 分钟线同一条数据要带时刻
        let ks_min = parse_klines("600519", Period::Min5, &data);
        assert!(ks_min[0].date.contains(':'));
    }

    #[test]
    fn kline_rows_missing_close_are_skipped() {
        let data: Value = serde_json::from_str(
            r#"{"column":["timestamp","close"],
                "item":[[1735660800000,null],[1735747200000,10.0],"garbage"]}"#,
        )
        .unwrap();
        let ks = parse_klines("600519", Period::Day, &data);
        assert_eq!(ks.len(), 1);
        assert_eq!(ks[0].close, 10.0);
    }

    #[test]
    fn compact_date_conversion() {
        assert_eq!(compact_to_ms("20240102"), date::to_epoch_ms("2024-01-02"));
        assert_eq!(compact_to_ms("0"), None); // "0" = 不限,交给 EPOCH_FLOOR
        assert_eq!(compact_to_ms("2024-01-02"), None);
        assert_eq!(compact_to_ms("2024010"), None);
        assert_eq!(compact_to_ms("abcdefgh"), None);
    }

    #[tokio::test]
    #[ignore] // 联网烟雾测试:cargo test -- --ignored
    async fn xueqiu_quote_live() {
        let q = Xueqiu.quote("600519", Market::SH).await.unwrap();
        assert!(q.price > 0.0 && q.high >= q.low);
        assert_eq!(q.code, "600519");
    }

    #[tokio::test]
    #[ignore]
    async fn xueqiu_batch_live() {
        let qs = Xueqiu
            .quotes(&[
                ("600519".into(), Market::SH),
                ("000858".into(), Market::SZ),
                ("00700".into(), Market::HK),
            ])
            .await
            .unwrap();
        assert_eq!(qs.len(), 3);
        assert!(qs.iter().all(|q| q.price > 0.0 && q.prev_close > 0.0));
    }

    /// 需要 XUEQIU_TOKEN;未设置时断言给出的是可操作的提示而不是解析错误。
    #[tokio::test]
    #[ignore]
    async fn xueqiu_kline_live_or_clear_hint() {
        let r = Xueqiu
            .klines("600519", Market::SH, Period::Day, "20240101", "20240201")
            .await;
        match (r, token()) {
            (Ok((_, ks)), _) => {
                assert!(!ks.is_empty());
                assert!(ks[0].close > 0.0);
                assert!(ks.windows(2).all(|w| w[0].date <= w[1].date));
            }
            (Err(e), None) => assert!(e.to_string().contains(TOKEN_ENV)),
            (Err(e), Some(_)) => panic!("已配置 token 仍失败:{e}"),
        }
    }
}
