use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stock {
    pub code: String,
    pub name: String,
    pub market: Market,
    pub added_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Market {
    SH,
    SZ,
}

impl Market {
    pub fn as_str(&self) -> &'static str {
        match self {
            Market::SH => "SH",
            Market::SZ => "SZ",
        }
    }

    pub fn from_str(s: &str) -> Option<Market> {
        match s {
            "SH" => Some(Market::SH),
            "SZ" => Some(Market::SZ),
            _ => None,
        }
    }

    /// 东财 secid 的市场前缀：沪市 1，深市 0。
    pub fn secid_prefix(&self) -> u8 {
        match self {
            Market::SH => 1,
            Market::SZ => 0,
        }
    }
}

/// 根据代码前缀推断所属市场，覆盖股票与场内基金（ETF/LOF）。
/// 沪市(secid 前缀 1)：6 股票 / 688 科创 / 9 B股 / 5 基金(50/51/52/56/58) / 11 转债
/// 深市(secid 前缀 0)：0 股票 / 3 创业板 / 2 B股 / 15·16·18 基金(LOF/ETF) / 12 转债
pub fn infer_market(code: &str) -> Option<Market> {
    match code.chars().next()? {
        '6' | '9' | '5' => Some(Market::SH),
        '0' | '2' | '3' => Some(Market::SZ),
        // 1 开头：沪市转债 11x，其余(深市基金 15/16/18、深市债 12)归深市
        '1' => {
            if code.starts_with("11") {
                Some(Market::SH)
            } else {
                Some(Market::SZ)
            }
        }
        _ => None,
    }
}

/// 东财请求用的 secid，如 "1.600519"。
pub fn secid(code: &str, market: Market) -> String {
    format!("{}.{}", market.secid_prefix(), code)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KLine {
    pub code: String,
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub amount: f64,
    pub turnover: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub code: String,
    pub name: String,
    pub price: f64,
    pub change: f64,
    pub change_pct: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub prev_close: f64,
    pub volume: f64,
    pub amount: f64,
    pub turnover: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: i64,
    pub code: String,
    pub price: f64,
    pub quantity: i64,
    pub date: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: i64,
    pub code: String,
    pub action: String,
    pub price: f64,
    pub quantity: i64,
    pub date: String,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_market_stocks_and_funds() {
        assert_eq!(infer_market("600519"), Some(Market::SH)); // 沪市股票
        assert_eq!(infer_market("000858"), Some(Market::SZ)); // 深市股票
        assert_eq!(infer_market("300750"), Some(Market::SZ)); // 创业板
        assert_eq!(infer_market("161226"), Some(Market::SZ)); // 深市 LOF
        assert_eq!(infer_market("159915"), Some(Market::SZ)); // 深市 ETF
        assert_eq!(infer_market("510300"), Some(Market::SH)); // 沪市 ETF
        assert_eq!(infer_market("113050"), Some(Market::SH)); // 沪市转债
        assert_eq!(infer_market("abc"), None);
    }

    #[test]
    fn secid_format() {
        assert_eq!(secid("161226", Market::SZ), "0.161226");
        assert_eq!(secid("600519", Market::SH), "1.600519");
    }
}
