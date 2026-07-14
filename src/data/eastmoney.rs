use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::models::{secid, KLine, Market, Quote};
use super::source::{http_client, KlineSource, QuoteSource};

const KLINE_URL: &str = "https://push2his.eastmoney.com/api/qt/stock/kline/get";
const QUOTE_URL: &str = "https://push2.eastmoney.com/api/qt/stock/get";
const ULIST_URL: &str = "https://push2.eastmoney.com/api/qt/ulist.np/get";

pub struct Eastmoney;

#[async_trait]
impl KlineSource for Eastmoney {
    fn name(&self) -> &'static str {
        "eastmoney"
    }

    /// 拉取日K线（前复权）。东财 klines 字段顺序由 fields2 决定，这里固定为：
    /// 日期,开,收,高,低,成交量,成交额,振幅,涨跌幅,涨跌额,换手率
    async fn klines(
        &self,
        code: &str,
        market: Market,
        beg: &str,
        end: &str,
    ) -> Result<(String, Vec<KLine>)> {
        let secid = secid(code, market);
        let resp = http_client()?
            .get(KLINE_URL)
            .query(&[
                ("secid", secid.as_str()),
                ("fields1", "f1,f2,f3,f4,f5,f6"),
                ("fields2", "f51,f52,f53,f54,f55,f56,f57,f58,f59,f60,f61"),
                ("klt", "101"),
                ("fqt", "1"),
                ("beg", beg),
                ("end", end),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析东财K线响应失败")?;
        let data = json
            .get("data")
            .filter(|d| !d.is_null())
            .ok_or_else(|| anyhow!("东财返回空数据，代码 {} 可能不存在", code))?;

        let name = data
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let empty = Vec::new();
        let raw = data
            .get("klines")
            .and_then(Value::as_array)
            .unwrap_or(&empty);

        let mut out = Vec::with_capacity(raw.len());
        for item in raw {
            let s = item.as_str().unwrap_or("");
            let f: Vec<&str> = s.split(',').collect();
            if f.len() < 11 {
                continue;
            }
            out.push(KLine {
                code: code.to_string(),
                date: f[0].to_string(),
                open: parse_f(f[1]),
                close: parse_f(f[2]),
                high: parse_f(f[3]),
                low: parse_f(f[4]),
                volume: parse_f(f[5]),
                amount: parse_f(f[6]),
                turnover: f[10].parse::<f64>().ok(),
            });
        }
        Ok((name, out))
    }
}

#[async_trait]
impl QuoteSource for Eastmoney {
    fn name(&self) -> &'static str {
        "eastmoney"
    }

    /// 实时行情。东财价格字段按 f59（小数位数）缩放。
    async fn quote(&self, code: &str, market: Market) -> Result<Quote> {
        let secid = secid(code, market);
        let resp = http_client()?
            .get(QUOTE_URL)
            .query(&[
                ("secid", secid.as_str()),
                (
                    "fields",
                    "f43,f44,f45,f46,f47,f48,f57,f58,f59,f60,f168,f169,f170",
                ),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析东财行情响应失败")?;
        let data = json
            .get("data")
            .filter(|d| !d.is_null())
            .ok_or_else(|| anyhow!("东财行情返回空，代码 {} 可能不存在", code))?;

        let decimals = data.get("f59").and_then(Value::as_i64).unwrap_or(2) as u32;
        let scale = 10f64.powi(decimals as i32);
        let price = |k: &str| data.get(k).and_then(Value::as_f64).unwrap_or(0.0) / scale;

        Ok(Quote {
            code: code.to_string(),
            name: data
                .get("f58")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            price: price("f43"),
            high: price("f44"),
            low: price("f45"),
            open: price("f46"),
            prev_close: price("f60"),
            change: price("f169"),
            change_pct: data.get("f170").and_then(Value::as_f64).unwrap_or(0.0) / 10000.0,
            volume: data.get("f47").and_then(Value::as_f64).unwrap_or(0.0),
            amount: data.get("f48").and_then(Value::as_f64).unwrap_or(0.0),
            turnover: data.get("f168").and_then(Value::as_f64).map(|v| v / 100.0),
        })
    }

    /// 东财 ulist.np 批量行情，secids 逗号分隔一次拿多只。价格字段按 100 缩放。
    async fn quotes(&self, reqs: &[(String, Market)]) -> Result<Vec<Quote>> {
        if reqs.is_empty() {
            return Ok(Vec::new());
        }
        let secids = reqs
            .iter()
            .map(|(c, m)| secid(c, *m))
            .collect::<Vec<_>>()
            .join(",");
        let resp = http_client()?
            .get(ULIST_URL)
            .query(&[
                // f1 = 价格小数位数（股票 2、基金常见 3），价格字段需按它缩放
                ("fields", "f1,f2,f3,f4,f5,f6,f8,f12,f14,f15,f16,f17,f18"),
                ("secids", secids.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析东财批量行情失败")?;
        let empty = Vec::new();
        let diff = json
            .get("data")
            .and_then(|d| d.get("diff"))
            .and_then(Value::as_array)
            .unwrap_or(&empty);

        let mut out = Vec::with_capacity(diff.len());
        for d in diff {
            let num = |k: &str| d.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let decimals = d.get("f1").and_then(Value::as_i64).unwrap_or(2) as i32;
            let scale = 10f64.powi(decimals);
            let price = |k: &str| num(k) / scale;
            out.push(Quote {
                code: d
                    .get("f12")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                name: d
                    .get("f14")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                price: price("f2"),
                prev_close: price("f18"),
                open: price("f17"),
                high: price("f15"),
                low: price("f16"),
                change: price("f4"),
                change_pct: num("f3") / 10000.0,
                volume: num("f5"),
                amount: num("f6"),
                turnover: d.get("f8").and_then(Value::as_f64).map(|v| v / 100.0),
            });
        }
        Ok(out)
    }
}

fn parse_f(s: &str) -> f64 {
    s.parse::<f64>().unwrap_or(0.0)
}
