use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::models::{KLine, Market, Quote};
use super::source::{http_client, KlineSource, QuoteSource};

pub struct Tencent;

fn prefix(m: Market) -> &'static str {
    match m {
        Market::SH => "sh",
        Market::SZ => "sz",
        Market::HK => "hk",
    }
}

/// "YYYYMMDD" -> "YYYY-MM-DD"，"0" -> ""。
fn dash_date(s: &str) -> String {
    if s == "0" || s.len() != 8 {
        return String::new();
    }
    format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8])
}

/// 解析单条 `v_XXX="1~名称~代码~..."` 的引号内内容为 Quote。
/// 3 现价,4 昨收,5 今开,6 成交量,33 最高,34 最低,37 成交额,38 换手率(%)
/// A股成交额单位是万元(×10000);港股 field[37] 已是原始 HKD,不乘。
fn parse_line(code: &str, inner: &str, is_hk: bool) -> Option<Quote> {
    let f: Vec<&str> = inner.split('~').collect();
    if f.len() < 6 || f[3].is_empty() {
        return None;
    }
    let g = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let price = g(3);
    let prev_close = g(4);
    let change = price - prev_close;
    Some(Quote {
        code: code.to_string(),
        name: f[1].to_string(),
        price,
        prev_close,
        open: g(5),
        high: if f.len() > 33 { g(33) } else { price },
        low: if f.len() > 34 { g(34) } else { price },
        change,
        change_pct: if prev_close != 0.0 {
            change / prev_close
        } else {
            0.0
        },
        volume: g(6),
        amount: if f.len() > 37 {
            if is_hk {
                g(37)
            } else {
                g(37) * 10000.0
            }
        } else {
            0.0
        },
        turnover: f.get(38).and_then(|s| s.parse::<f64>().ok()),
        pe: None,
        pb: None,
    })
}

#[async_trait]
impl QuoteSource for Tencent {
    fn name(&self) -> &'static str {
        "tencent"
    }

    async fn quote(&self, code: &str, market: Market) -> Result<Quote> {
        let mut v = self.quotes(&[(code.to_string(), market)]).await?;
        v.pop()
            .ok_or_else(|| anyhow!("腾讯行情无数据，代码 {} 可能不存在", code))
    }

    /// 腾讯 gtimg 原生支持一次查多只：q=sh600519,sz000858。GBK 编码，~ 分隔。
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
            .get(format!("https://qt.gtimg.cn/q={list}"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);

        let mut out = Vec::new();
        for line in text.lines() {
            // v_sh600519="1~名称~...";
            let Some(sym) = line
                .split_once("v_")
                .and_then(|(_, r)| r.split_once('=').map(|(l, _)| l))
            else {
                continue;
            };
            let code = sym.get(2..).unwrap_or(sym); // 去掉 sh/sz/hk 前缀
            let Some(inner) = line
                .split_once('"')
                .and_then(|(_, r)| r.rsplit_once('"').map(|(l, _)| l))
            else {
                continue;
            };
            if let Some(q) = parse_line(code, inner, sym.starts_with("hk")) {
                out.push(q);
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl KlineSource for Tencent {
    fn name(&self) -> &'static str {
        "tencent"
    }

    /// 腾讯前复权日K，支持区间但单次上限约 640 条。
    async fn klines(
        &self,
        code: &str,
        market: Market,
        beg: &str,
        end: &str,
    ) -> Result<(String, Vec<KLine>)> {
        let sym = format!("{}{}", prefix(market), code);
        let param = format!("{},day,{},{},640,qfq", sym, dash_date(beg), dash_date(end));
        let url = "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get";
        let resp = http_client()?
            .get(url)
            .query(&[("param", param.as_str())])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析腾讯K线响应失败")?;
        let node = json
            .get("data")
            .and_then(|d| d.get(&sym))
            .ok_or_else(|| anyhow!("腾讯K线返回空，代码 {} 可能不存在", code))?;
        let arr = node
            .get("qfqday")
            .or_else(|| node.get("day"))
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("腾讯K线缺少日K字段，代码 {}", code))?;

        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let c = row.as_array();
            let Some(c) = c else { continue };
            if c.len() < 6 {
                continue;
            }
            let s = |i: usize| c[i].as_str().unwrap_or("");
            let n = |i: usize| s(i).parse::<f64>().unwrap_or(0.0);
            out.push(KLine {
                code: code.to_string(),
                date: s(0).to_string(),
                open: n(1),
                close: n(2),
                high: n(3),
                low: n(4),
                volume: n(5),
                amount: 0.0,
                turnover: None,
            });
        }
        Ok((String::new(), out))
    }
}
