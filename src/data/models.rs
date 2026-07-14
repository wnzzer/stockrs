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

/// 根据 A 股代码推断所属市场。
/// 6 开头为沪市，0/3 开头为深市（含创业板 300）。
pub fn infer_market(code: &str) -> Option<Market> {
    match code.chars().next()? {
        '6' | '9' => Some(Market::SH),
        '0' | '3' | '2' => Some(Market::SZ),
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
