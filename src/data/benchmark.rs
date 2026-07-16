//! 基准指数取数。指数不能走 infer_market(000300 会被判成深市,实为沪市 secid
//! 1.000300),也不走三源故障切换(腾讯/新浪对指数代码无效),而是别名硬编码
//! market 后直连东财 kline 接口。取数失败时返回 None,由调用方降级(跳过基准对比)。

use super::eastmoney::Eastmoney;
use super::models::{normalize_code, KLine, Market};
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

/// CLI 代码入口解析,在股票/基金/港股之外额外识别指数:
/// - 交易所前缀 `sh000001`/`sz399001` → 强制市场,消解指数与同码股票的歧义
///   (上证指数 secid `1.000001` vs 平安银行 `0.000001`)。
/// - 指数别名/中文名 `上证指数`/`sh`/`hs300`/`kc50` → 走 [`resolve`] 别名表。
/// - 其余(含裸 6 位数字)交回 [`normalize_code`]:裸 `000001` 仍是平安银行,不抢成指数。
pub fn resolve_input(input: &str) -> Option<(String, Market)> {
    let t = input.trim();
    if let Some(r) = parse_exchange_prefix(t) {
        return Some(r);
    }
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return normalize_code(t);
    }
    if let Some((code, _, market)) = resolve(t) {
        return Some((code.to_string(), market));
    }
    normalize_code(t)
}

/// 解析 `shXXXXXX`/`szXXXXXX`(X 为 6 位数字)显式前缀 → (代码, 市场)。
/// `sh`/`sz` 单独(其后非 6 位数字)不算前缀,让位给别名表(`sh` = 上证指数)。
fn parse_exchange_prefix(input: &str) -> Option<(String, Market)> {
    let lower = input.to_lowercase();
    let market = if lower.starts_with("sh") {
        Market::SH
    } else if lower.starts_with("sz") {
        Market::SZ
    } else {
        return None;
    };
    let rest = &lower[2..];
    (rest.len() == 6 && rest.bytes().all(|b| b.is_ascii_digit()))
        .then(|| (rest.to_string(), market))
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

    #[test]
    fn resolve_input_index_vs_same_coded_stock() {
        // 交易所前缀强制市场:区分同码指数与股票
        let sh = |c: &str| Some((c.to_string(), Market::SH));
        let sz = |c: &str| Some((c.to_string(), Market::SZ));
        assert_eq!(resolve_input("sh000001"), sh("000001")); // 上证指数,非平安银行
        assert_eq!(resolve_input("SH000688"), sh("000688")); // 科创50,大小写不敏感
        assert_eq!(resolve_input("sz399001"), sz("399001")); // 深证成指
        assert_eq!(resolve_input("sh600519"), sh("600519")); // 前缀对普通股也生效

        // 指数别名/中文名
        assert_eq!(resolve_input("上证指数"), sh("000001"));
        assert_eq!(resolve_input("hs300"), sh("000300"));
        assert_eq!(resolve_input("kc50"), sh("000688"));
        assert_eq!(resolve_input("sh"), sh("000001")); // 单独 sh = 上证指数

        // 裸 6 位数字仍当股票,不被同码指数别名抢走
        assert_eq!(resolve_input("000001"), sz("000001")); // 平安银行
        assert_eq!(resolve_input("000688"), sz("000688")); // 国城矿业
        assert_eq!(resolve_input("600519"), sh("600519"));

        // 港股与无效输入透传给 normalize_code
        assert_eq!(resolve_input("hk00700"), Some(("00700".into(), Market::HK)));
        assert!(resolve_input("无效").is_none());
    }
}
