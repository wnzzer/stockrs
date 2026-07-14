use anyhow::{anyhow, Result};
use comfy_table::Table;

use crate::data::models::infer_market;
use crate::data::source;

pub async fn run(codes: Vec<String>) -> Result<()> {
    if codes.is_empty() {
        return Err(anyhow!("请提供至少一个股票代码"));
    }
    let mut reqs = Vec::with_capacity(codes.len());
    for code in &codes {
        let market = infer_market(code).ok_or_else(|| anyhow!("无法识别的股票代码 {}", code))?;
        reqs.push((code.clone(), market));
    }

    let (quotes, failed) = source::fetch_quotes(&reqs).await?;

    let mut table = Table::new();
    table.set_header(vec![
        "代码",
        "名称",
        "现价",
        "涨跌",
        "涨跌幅",
        "今开",
        "最高",
        "最低",
        "昨收",
        "来源",
    ]);
    for (q, src) in quotes {
        table.add_row(vec![
            q.code,
            q.name,
            format!("{:.2}", q.price),
            format!("{:+.2}", q.change),
            format!("{:+.2}%", q.change_pct * 100.0),
            format!("{:.2}", q.open),
            format!("{:.2}", q.high),
            format!("{:.2}", q.low),
            format!("{:.2}", q.prev_close),
            src.to_string(),
        ]);
    }
    println!("{table}");
    if !failed.is_empty() {
        eprintln!("以下代码所有数据源均无数据：{}", failed.join(", "));
    }
    Ok(())
}
