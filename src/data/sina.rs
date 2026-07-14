use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::models::{KLine, Market, Quote};
use super::source::{http_client, KlineSource, QuoteSource};

pub struct Sina;

fn prefix(m: Market) -> &'static str {
    match m {
        Market::SH => "sh",
        Market::SZ => "sz",
    }
}

/// 解析单行 `var hq_str_XXX="..."` 的引号内内容为 Quote。
/// 0 名称,1 今开,2 昨收,3 现价,4 最高,5 最低,8 成交量(股),9 成交额(元)
fn parse_line(code: &str, inner: &str) -> Option<Quote> {
    let f: Vec<&str> = inner.split(',').collect();
    if f.len() < 10 || f[3].is_empty() {
        return None;
    }
    let g = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let price = g(3);
    let prev_close = g(2);
    let change = price - prev_close;
    Some(Quote {
        code: code.to_string(),
        name: f[0].to_string(),
        price,
        prev_close,
        open: g(1),
        high: g(4),
        low: g(5),
        change,
        change_pct: if prev_close != 0.0 {
            change / prev_close
        } else {
            0.0
        },
        volume: g(8) / 100.0, // 股 -> 手
        amount: g(9),
        turnover: None,
    })
}

#[async_trait]
impl QuoteSource for Sina {
    fn name(&self) -> &'static str {
        "sina"
    }

    async fn quote(&self, code: &str, market: Market) -> Result<Quote> {
        let mut v = self.quotes(&[(code.to_string(), market)]).await?;
        v.pop()
            .ok_or_else(|| anyhow!("新浪行情无数据，代码 {} 可能停牌或不存在", code))
    }

    /// 新浪 hq.sinajs.cn 原生支持一次查多只：list=sh600519,sz000858。需带 Referer，GBK 编码。
    async fn quotes(&self, reqs: &[(String, Market)]) -> Result<Vec<Quote>> {
        if reqs.is_empty() {
            return Ok(Vec::new());
        }
        let list = reqs
            .iter()
            .map(|(c, m)| format!("{}{}", prefix(*m), c))
            .collect::<Vec<_>>()
            .join(",");
        let bytes = http_client()?
            .get(format!("https://hq.sinajs.cn/list={list}"))
            .header("Referer", "https://finance.sina.com.cn")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);

        let mut out = Vec::new();
        for line in text.lines() {
            // var hq_str_sh600519="名称,...";
            let Some(sym) = line
                .split_once("hq_str_")
                .and_then(|(_, r)| r.split_once('=').map(|(l, _)| l))
            else {
                continue;
            };
            let code = sym.get(2..).unwrap_or(sym); // 去掉 sh/sz 前缀
            let Some(inner) = line
                .split_once('"')
                .and_then(|(_, r)| r.rsplit_once('"').map(|(l, _)| l))
            else {
                continue;
            };
            if let Some(q) = parse_line(code, inner) {
                out.push(q);
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl KlineSource for Sina {
    fn name(&self) -> &'static str {
        "sina"
    }

    /// 新浪日K（非前复权），仅返回最近约 1023 条，本地按区间过滤。
    async fn klines(
        &self,
        code: &str,
        market: Market,
        beg: &str,
        end: &str,
    ) -> Result<(String, Vec<KLine>)> {
        let sym = format!("{}{}", prefix(market), code);
        let url =
            "https://money.finance.sina.com.cn/quotes_service/api/json_v2.php/CN_MarketData.getKLineData";
        let resp = http_client()?
            .get(url)
            .query(&[
                ("symbol", sym.as_str()),
                ("scale", "240"),
                ("ma", "no"),
                ("datalen", "1023"),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析新浪K线响应失败")?;
        let arr = json
            .as_array()
            .ok_or_else(|| anyhow!("新浪K线返回异常，代码 {} 可能不存在", code))?;

        let beg_d = dash_date(beg);
        let end_d = dash_date(end);
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let day = item.get("day").and_then(Value::as_str).unwrap_or("");
            if day.is_empty() {
                continue;
            }
            if !beg_d.is_empty() && day < beg_d.as_str() {
                continue;
            }
            if !end_d.is_empty() && day > end_d.as_str() {
                continue;
            }
            let n = |k: &str| {
                item.get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            out.push(KLine {
                code: code.to_string(),
                date: day.to_string(),
                open: n("open"),
                close: n("close"),
                high: n("high"),
                low: n("low"),
                volume: n("volume") / 100.0, // 股 -> 手
                amount: 0.0,
                turnover: None,
            });
        }
        Ok((String::new(), out))
    }
}

/// "YYYYMMDD" -> "YYYY-MM-DD"，"0" -> ""。
fn dash_date(s: &str) -> String {
    if s == "0" || s.len() != 8 {
        return String::new();
    }
    format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8])
}
