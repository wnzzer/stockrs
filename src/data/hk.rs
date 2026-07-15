//! 港股专用取数:东财 F10(每手股数)+ 百度股市通(日频估值 PE/PB/市值)。
//! datacenter.eastmoney.com 独立于 push2;百度 finance.baidu.com/opendata 免登录直连。
//! 东财 A股估值表(RPT_VALUEANALYSIS_DET)不含港股,故港股 PE/PB 改走百度。

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::Value;

use super::models::Fundamental;
use super::source::http_client;

const F10_URL: &str = "https://datacenter.eastmoney.com/securities/api/data/v1/get";
const BAIDU_URL: &str = "https://finance.baidu.com/opendata";

/// 取港股每手股数(RPT_HKF10_INFO_SECURITYINFO 的 TRADE_UNIT)。
/// 失败/无数据返回 None,由调用方回退默认 100。code 为 5 位港股代码(如 "00700")。
pub async fn fetch_lot_size(code: &str) -> Result<Option<i64>> {
    let filter = format!("(SECUCODE=\"{}.HK\")", code);
    let resp = http_client()?
        .get(F10_URL)
        .query(&[
            ("reportName", "RPT_HKF10_INFO_SECURITYINFO"),
            ("columns", "SECUCODE,TRADE_UNIT"),
            ("filter", filter.as_str()),
            ("client", "PC"),
            ("source", "F10"),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let json: Value = serde_json::from_str(&resp).context("解析港股 F10 响应失败")?;
    let lot = json
        .get("result")
        .filter(|r| !r.is_null())
        .and_then(|r| r.get("data"))
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .and_then(|row| row.get("TRADE_UNIT"))
        .and_then(Value::as_i64);
    Ok(lot)
}

/// 港股日频估值(百度股市通)。取近三年 PE(TTM)/PB/总市值,按日期合并成 Fundamental,
/// 落进与 A股同一张 fundamentals 表(下游对齐/回测逻辑完全复用)。code 为 5 位港股代码。
/// 三个指标各请求一次,单个失败不致命——尽力返回已拿到的字段。
pub async fn fetch_valuation(code: &str) -> Result<Vec<Fundamental>> {
    let pe = fetch_metric(code, "市盈率(TTM)").await.unwrap_or_default();
    let pb = fetch_metric(code, "市净率").await.unwrap_or_default();
    let mv = fetch_metric(code, "总市值").await.unwrap_or_default();

    // BTreeMap 按日期键天然升序,与 store::get_fundamentals(ORDER BY date ASC)一致。
    let mut map: BTreeMap<String, Fundamental> = BTreeMap::new();
    for (d, v) in pe {
        map.entry(d.clone())
            .or_insert_with(|| blank(code, &d))
            .pe_ttm = Some(v);
    }
    for (d, v) in pb {
        map.entry(d.clone())
            .or_insert_with(|| blank(code, &d))
            .pb_mrq = Some(v);
    }
    for (d, v) in mv {
        // 百度总市值单位是亿港元 → ×1e8 转港元,与 A股 total_mv(元)口径统一。
        map.entry(d.clone())
            .or_insert_with(|| blank(code, &d))
            .total_mv = Some(v * 1e8);
    }
    Ok(map.into_values().collect())
}

fn blank(code: &str, date: &str) -> Fundamental {
    Fundamental {
        code: code.to_string(),
        date: date.to_string(),
        pe_ttm: None,
        pb_mrq: None,
        // 百度“市现率”是市值/现金流,非市销率,港股 PS 留空。
        ps_ttm: None,
        total_mv: None,
    }
}

/// 取单个指标近三年的 (date, value) 序列。百度按“指标中文名”查询,免登录。
/// 响应结构变动/无数据 → 返回空(由 fetch_valuation 降级),不 panic。
async fn fetch_metric(code: &str, metric: &str) -> Result<Vec<(String, f64)>> {
    let resp = http_client()?
        .get(BAIDU_URL)
        .query(&[
            ("openapi", "1"),
            ("dspName", "iphone"),
            ("tn", "tangram"),
            ("client", "app"),
            ("query", metric),
            ("tag", metric),
            ("code", code),
            ("word", ""),
            ("resource_id", "51171"),
            ("market", "hk"),
            ("chart_select", "近三年"),
            ("skip_industry", "1"),
            ("finClientType", "pc"),
        ])
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let json: Value = serde_json::from_str(&resp).context("解析百度估值响应失败")?;
    // 路径:Result[0].DisplayData.resultData.tplData.result.chartInfo[0].body = [[date,value],...]
    let body = json
        .get("Result")
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("DisplayData"))
        .and_then(|r| r.get("resultData"))
        .and_then(|r| r.get("tplData"))
        .and_then(|r| r.get("result"))
        .and_then(|r| r.get("chartInfo"))
        .and_then(|r| r.get(0))
        .and_then(|r| r.get("body"))
        .and_then(Value::as_array);

    let mut out = Vec::new();
    if let Some(rows) = body {
        for row in rows {
            let Some(arr) = row.as_array() else { continue };
            if arr.len() < 2 {
                continue;
            }
            let date = arr[0].as_str().unwrap_or("");
            if date.len() < 10 {
                continue;
            }
            // value 是字符串数字;"-"/空/非数字(该日该指标缺失)→ 跳过。
            if let Some(v) = arr[1].as_str().and_then(|s| s.parse::<f64>().ok()) {
                out.push((date[..10].to_string(), v));
            }
        }
    }
    Ok(out)
}
