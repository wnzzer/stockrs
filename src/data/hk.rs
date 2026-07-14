//! 港股专用取数(东财 F10 证券资料)。目前只取每手股数 TRADE_UNIT。
//! 主机 datacenter.eastmoney.com 独立于 push2,未见限流。

use anyhow::{Context, Result};
use serde_json::Value;

use super::source::http_client;

const F10_URL: &str = "https://datacenter.eastmoney.com/securities/api/data/v1/get";

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
