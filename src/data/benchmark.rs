//! 基准指数取数。指数不能走 infer_market(000300 会被判成深市,实为沪市 secid
//! 1.000300),也不走三源故障切换(腾讯/新浪对指数代码无效),而是别名硬编码
//! market 后直连东财 kline 接口。取数失败时返回 None,由调用方降级(跳过基准对比)。

use super::eastmoney::Eastmoney;
use super::models::{KLine, Market};
use super::source::KlineSource;
use super::Store;

/// 把用户输入(别名或代码)解析为 (指数代码, 显示名, 市场)。未知返回 None。
pub fn resolve(input: &str) -> Option<(&'static str, &'static str, Market)> {
    let s = input.trim().to_lowercase();
    let r = match s.as_str() {
        "hs300" | "000300" | "沪深300" | "沪深300指数" => ("000300", "沪深300", Market::SH),
        "zz500" | "000905" | "中证500" => ("000905", "中证500", Market::SH),
        "zz1000" | "000852" | "中证1000" => ("000852", "中证1000", Market::SH),
        "sh" | "sse" | "000001" | "上证指数" | "上证" => ("000001", "上证指数", Market::SH),
        "sz" | "399001" | "深证成指" | "深证" => ("399001", "深证成指", Market::SZ),
        "cyb" | "399006" | "创业板指" | "创业板" => ("399006", "创业板指", Market::SZ),
        "kc50" | "000688" | "科创50" => ("000688", "科创50", Market::SH),
        _ => return None,
    };
    Some(r)
}

/// 取基准日K:先读本地库,不足则直连东财(硬编码 market)并缓存回库。
/// 返回 (代码, 名称, 日K)。任何环节失败/为空返回 None。
pub async fn fetch(
    store: &mut Store,
    input: &str,
    start: Option<&str>,
    end: Option<&str>,
) -> Option<(String, String, Vec<KLine>)> {
    let (code, name, market) = resolve(input)?;

    if let Ok(ks) = store.get_klines(code, start, end) {
        if ks.len() >= 2 {
            return Some((code.to_string(), name.to_string(), ks));
        }
    }

    let beg = start
        .map(|s| s.replace('-', ""))
        .unwrap_or_else(|| "0".into());
    let e = end
        .map(|s| s.replace('-', ""))
        .unwrap_or_else(|| "20500101".into());
    match Eastmoney.klines(code, market, &beg, &e).await {
        Ok((_, ks)) if !ks.is_empty() => {
            let _ = store.upsert_klines(&ks);
            let filtered = store.get_klines(code, start, end).unwrap_or(ks);
            Some((code.to_string(), name.to_string(), filtered))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_aliases_and_market() {
        assert_eq!(resolve("hs300"), Some(("000300", "沪深300", Market::SH)));
        assert_eq!(resolve("000300"), Some(("000300", "沪深300", Market::SH)));
        assert_eq!(resolve("ZZ500"), Some(("000905", "中证500", Market::SH)));
        assert_eq!(resolve("cyb"), Some(("399006", "创业板指", Market::SZ)));
        assert_eq!(resolve("sz"), Some(("399001", "深证成指", Market::SZ)));
        assert!(resolve("不存在").is_none());
    }
}
