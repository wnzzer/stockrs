//! 东财 datacenter 历史估值(日度 PE/PB/PS/总市值)。
//!
//! 接口 RPT_VALUEANALYSIS_DET 返回真实浮点(不缩放),PE 可为负(亏损),
//! Content-Type 是 text/plain 故用 .text() 解析;result 为 null 表无数据/出错,
//! 按空处理由调用方降级。SECUCODE 格式是 "600519.SH"(见 models::secu_code)。

use anyhow::{Context, Result};
use serde_json::Value;

use super::models::{secu_code, Fundamental, Market};
use super::source::http_client;

const URL: &str = "https://datacenter-web.eastmoney.com/api/data/v1/get";
// 单只完整历史约 2000-2100 行,一页足够;仍读 pages 兜底分页。
const PAGE_SIZE: usize = 5000;

/// 拉取某股历史估值。since 为 "YYYY-MM-DD" 时只取该日(含)之后,用于增量。
pub async fn fetch(code: &str, market: Market, since: Option<&str>) -> Result<Vec<Fundamental>> {
    // 港股不在 A 股 RPT_VALUEANALYSIS_DET 表里(会永远返回空),后续阶段另接 HK F10 源。
    if market == Market::HK {
        return Ok(Vec::new());
    }
    let secu = secu_code(code, market);
    // 日期值必须单引号包裹;多个括号子句是 AND。reqwest .query() 会正确百分号编码。
    let mut filter = format!("(SECUCODE=\"{}\")", secu);
    if let Some(s) = since {
        filter.push_str(&format!("(TRADE_DATE>='{}')", s));
    }

    let mut out = Vec::new();
    let mut page = 1usize;
    loop {
        let page_str = page.to_string();
        let ps = PAGE_SIZE.to_string();
        let resp = http_client()?
            .get(URL)
            .query(&[
                ("reportName", "RPT_VALUEANALYSIS_DET"),
                (
                    "columns",
                    "SECUCODE,TRADE_DATE,PE_TTM,PB_MRQ,PS_TTM,TOTAL_MARKET_CAP",
                ),
                ("filter", filter.as_str()),
                ("pageNumber", page_str.as_str()),
                ("pageSize", ps.as_str()),
                ("sortColumns", "TRADE_DATE"),
                ("sortTypes", "1"),
                ("source", "WEB"),
                ("client", "WEB"),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let json: Value = serde_json::from_str(&resp).context("解析东财基本面响应失败")?;
        // result 为 null:无数据(code 9201)或参数错误(9501),结束(通常返回空)。
        let result = match json.get("result").filter(|r| !r.is_null()) {
            Some(r) => r,
            None => break,
        };

        let empty = Vec::new();
        let data = result
            .get("data")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        for d in data {
            let date = d.get("TRADE_DATE").and_then(Value::as_str).unwrap_or("");
            if date.len() < 10 {
                continue;
            }
            out.push(Fundamental {
                code: code.to_string(),
                date: date[..10].to_string(), // "YYYY-MM-DD 00:00:00" -> "YYYY-MM-DD"
                // null / 缺字段 -> None;负数(亏损)保留 Some(负)。切勿 unwrap_or(0.0)。
                pe_ttm: d.get("PE_TTM").and_then(Value::as_f64),
                pb_mrq: d.get("PB_MRQ").and_then(Value::as_f64),
                ps_ttm: d.get("PS_TTM").and_then(Value::as_f64),
                total_mv: d.get("TOTAL_MARKET_CAP").and_then(Value::as_f64),
            });
        }

        let pages = result.get("pages").and_then(Value::as_u64).unwrap_or(1) as usize;
        if page >= pages {
            break;
        }
        page += 1;
    }
    Ok(out)
}

/// 对齐后的基本面序列(与交易日序列等长)。
pub struct Aligned {
    pub pe: Vec<f64>,
    pub pb: Vec<f64>,
    pub ps: Vec<f64>,
    pub mv: Vec<f64>,
}

/// 把基本面按 **on-or-before carry-forward** 对齐到交易日序列(两者都须升序)。
/// 某 bar 取 date<=该日的最近一条基本面;该条某字段为 None(亏损/缺失)则该 bar
/// 该指标为 NaN(不沿用更早的值);首条基本面之前为 NaN。保证无未来函数。
pub fn align(dates: &[String], funda: &[Fundamental]) -> Aligned {
    let n = dates.len();
    let mut pe = vec![f64::NAN; n];
    let mut pb = vec![f64::NAN; n];
    let mut ps = vec![f64::NAN; n];
    let mut mv = vec![f64::NAN; n];

    let mut j = 0usize;
    let mut cur: Option<&Fundamental> = None;
    for (i, d) in dates.iter().enumerate() {
        while j < funda.len() && funda[j].date.as_str() <= d.as_str() {
            cur = Some(&funda[j]);
            j += 1;
        }
        if let Some(f) = cur {
            pe[i] = f.pe_ttm.unwrap_or(f64::NAN);
            pb[i] = f.pb_mrq.unwrap_or(f64::NAN);
            ps[i] = f.ps_ttm.unwrap_or(f64::NAN);
            mv[i] = f.total_mv.unwrap_or(f64::NAN);
        }
    }
    Aligned { pe, pb, ps, mv }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(date: &str, pe: Option<f64>, ps: Option<f64>) -> Fundamental {
        Fundamental {
            code: "T".into(),
            date: date.into(),
            pe_ttm: pe,
            pb_mrq: Some(1.5),
            ps_ttm: ps,
            total_mv: Some(100.0),
        }
    }

    #[test]
    fn align_pit_carry_forward() {
        let funda = vec![
            f("2024-01-02", Some(10.0), None),
            f("2024-01-04", Some(20.0), Some(3.0)),
        ];
        let dates: Vec<String> = [
            "2024-01-01",
            "2024-01-02",
            "2024-01-03",
            "2024-01-04",
            "2024-01-05",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let a = align(&dates, &funda);

        assert!(a.pe[0].is_nan()); // 首条基本面之前
        assert_eq!(a.pe[1], 10.0); // d2 本条
        assert_eq!(a.pe[2], 10.0); // d3 carry d2(无未来函数,用的是 d2)
        assert_eq!(a.pe[3], 20.0); // d4 本条
        assert_eq!(a.pe[4], 20.0); // d5 carry d4
                                   // ps 在 d2/d3 那条为 None -> 该 bar NaN(不沿用);d4 起为 3.0
        assert!(a.ps[1].is_nan());
        assert!(a.ps[2].is_nan());
        assert_eq!(a.ps[3], 3.0);
        assert_eq!(a.pb[1], 1.5);
    }
}
