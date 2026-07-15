use anyhow::{anyhow, Result};
use comfy_table::Table;

use crate::data::models::normalize_code;
use crate::data::{source, Store};

pub async fn run(codes: Vec<String>) -> Result<()> {
    if codes.is_empty() {
        return Err(anyhow!("请提供至少一个股票代码"));
    }
    let mut reqs = Vec::with_capacity(codes.len());
    for input in &codes {
        let (code, market) =
            normalize_code(input).ok_or_else(|| anyhow!("无法识别的代码 {}", input))?;
        reqs.push((code, market));
    }

    let (quotes, failed) = source::fetch_quotes(&reqs).await?;

    // 实时源(腾讯/新浪)不带 PE/PB;缺失时回退本地基本面表(东财 datacenter / 百度,
    // 与行情主机独立,行情主机挂了它常仍可用)。best-effort:开库失败就跳过兜底。
    let store = Store::open_default().ok();

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
        "PE",
        "PB",
        "来源",
    ]);
    let fv = |o: Option<f64>, mark: &str| match o {
        Some(x) => format!("{:.2}{}", x, mark),
        None => "--".to_string(),
    };
    let mut used_local = false;
    for (q, src) in quotes {
        let (mut pe, mut pb) = (q.pe, q.pb);
        let (mut pe_mark, mut pb_mark) = ("", "");
        if pe.is_none() || pb.is_none() {
            if let Some(f) = store
                .as_ref()
                .and_then(|s| s.latest_fundamental(&q.code).ok().flatten())
            {
                if pe.is_none() && f.pe_ttm.is_some() {
                    pe = f.pe_ttm;
                    pe_mark = "*";
                    used_local = true;
                }
                if pb.is_none() && f.pb_mrq.is_some() {
                    pb = f.pb_mrq;
                    pb_mark = "*";
                    used_local = true;
                }
            }
        }
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
            fv(pe, pe_mark),
            fv(pb, pb_mark),
            src.to_string(),
        ]);
    }
    println!("{table}");
    if used_local {
        println!("* PE/PB 来自本地基本面（截至最近收盘），实时行情源未提供");
    }
    if !failed.is_empty() {
        eprintln!("以下代码所有数据源均无数据：{}", failed.join(", "));
    }
    Ok(())
}
