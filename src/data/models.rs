use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stock {
    pub code: String,
    pub name: String,
    pub market: Market,
    pub added_at: String,
    /// 每手股数(A股 100,港股逐股不同,由 F10 取得)。
    pub lot_size: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Market {
    SH,
    SZ,
    HK,
}

impl Market {
    pub fn as_str(&self) -> &'static str {
        match self {
            Market::SH => "SH",
            Market::SZ => "SZ",
            Market::HK => "HK",
        }
    }

    pub fn from_str(s: &str) -> Option<Market> {
        match s {
            "SH" => Some(Market::SH),
            "SZ" => Some(Market::SZ),
            "HK" => Some(Market::HK),
            _ => None,
        }
    }

    /// 东财 secid 的市场前缀：沪市 1，深市 0，港股 116。
    pub fn secid_prefix(&self) -> u8 {
        match self {
            Market::SH => 1,
            Market::SZ => 0,
            Market::HK => 116,
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

/// 把用户输入归一为 (标准代码, 市场)。
/// 港股:`hk` 前缀 / `.HK` 后缀 / ≤5 位纯数字 → 补零到 5 位 + HK(A股/基金均为 6 位,无冲突);
/// 6 位 → infer_market;其余 None。用于 CLI 入口识别港股 vs A股。
pub fn normalize_code(input: &str) -> Option<(String, Market)> {
    let up = input.trim().to_uppercase();
    let hk_explicit = up.starts_with("HK") || up.ends_with(".HK");
    let digits: String = up.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    if hk_explicit || digits.len() <= 5 {
        if digits.len() > 5 {
            return None; // 港股代码最多 5 位
        }
        Some((format!("{:0>5}", digits), Market::HK))
    } else if digits.len() == 6 {
        infer_market(&digits).map(|m| (digits, m))
    } else {
        None
    }
}

/// 东财请求用的 secid，如 "1.600519" / "116.00700"。
pub fn secid(code: &str, market: Market) -> String {
    format!("{}.{}", market.secid_prefix(), code)
}

/// 东财 datacenter 基本面接口用的 SECUCODE，如 "600519.SH" / "000858.SZ"。
/// 注意与 secid 格式完全不同(secid 是 "1.600519")。
pub fn secu_code(code: &str, market: Market) -> String {
    format!("{}.{}", code, market.as_str())
}

/// K线周期。日线与分钟线共用同一套存储/回测管线,差异由本枚举收口:
/// 各数据源的接口参数、DB 主键分区、年化因子都从这里取,不散落魔法值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Period {
    #[default]
    Day,
    Min1,
    Min5,
    Min15,
    Min30,
    Min60,
}

impl Period {
    /// 解析 CLI 输入。接受 d/1m/5m/15m/30m/60m 及常见别名。
    pub fn parse(s: &str) -> Option<Period> {
        match s.trim().to_ascii_lowercase().as_str() {
            "d" | "day" | "1d" | "daily" | "day1" => Some(Period::Day),
            "1m" | "1" | "m1" | "min1" => Some(Period::Min1),
            "5m" | "5" | "m5" | "min5" => Some(Period::Min5),
            "15m" | "15" | "m15" | "min15" => Some(Period::Min15),
            "30m" | "30" | "m30" | "min30" => Some(Period::Min30),
            "60m" | "60" | "m60" | "min60" | "1h" | "h1" => Some(Period::Min60),
            _ => None,
        }
    }

    /// DB 存储标签,也是 klines 主键的分区维度。
    pub fn tag(&self) -> &'static str {
        match self {
            Period::Day => "d",
            Period::Min1 => "m1",
            Period::Min5 => "m5",
            Period::Min15 => "m15",
            Period::Min30 => "m30",
            Period::Min60 => "m60",
        }
    }

    /// 人类可读标签(报告/日志)。
    pub fn label(&self) -> &'static str {
        match self {
            Period::Day => "日K",
            Period::Min1 => "1分",
            Period::Min5 => "5分",
            Period::Min15 => "15分",
            Period::Min30 => "30分",
            Period::Min60 => "60分",
        }
    }

    pub fn is_intraday(&self) -> bool {
        !matches!(self, Period::Day)
    }

    /// 东财 kline 接口的 klt 参数。
    pub fn eastmoney_klt(&self) -> &'static str {
        match self {
            Period::Day => "101",
            Period::Min1 => "1",
            Period::Min5 => "5",
            Period::Min15 => "15",
            Period::Min30 => "30",
            Period::Min60 => "60",
        }
    }

    /// 新浪 getKLineData 的 scale 参数(分钟数;日线用 240)。
    pub fn sina_scale(&self) -> &'static str {
        match self {
            Period::Day => "240",
            Period::Min1 => "1",
            Period::Min5 => "5",
            Period::Min15 => "15",
            Period::Min30 => "30",
            Period::Min60 => "60",
        }
    }

    /// 一年的 bar 数,用于年化(收益/夏普/Alpha)。A股每交易日约 240 分钟,
    /// 按 252 交易日折算;分钟线年化本身是近似,常数取整已足够。
    pub fn bars_per_year(&self) -> f64 {
        const TRADING_DAYS: f64 = 252.0;
        const MINUTES_PER_DAY: f64 = 240.0;
        let per_day = match self {
            Period::Day => return TRADING_DAYS,
            Period::Min1 => MINUTES_PER_DAY,
            Period::Min5 => MINUTES_PER_DAY / 5.0,
            Period::Min15 => MINUTES_PER_DAY / 15.0,
            Period::Min30 => MINUTES_PER_DAY / 30.0,
            Period::Min60 => MINUTES_PER_DAY / 60.0,
        };
        TRADING_DAYS * per_day
    }
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

/// 单日基本面估值(来自东财 datacenter)。字段可为 None:亏损股 PE 为负(仍是 Some),
/// 真正缺失/无数据时为 None——不要用 0.0 代替 None。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fundamental {
    pub code: String,
    pub date: String, // YYYY-MM-DD
    pub pe_ttm: Option<f64>,
    pub pb_mrq: Option<f64>,
    pub ps_ttm: Option<f64>,
    pub total_mv: Option<f64>, // 总市值(元)
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
    /// 市盈率(动/TTM)、市净率;东财与腾讯(A股+港股)提供,新浪为 None。
    pub pe: Option<f64>,
    pub pb: Option<f64>,
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
    fn period_parse_aliases_and_tags() {
        assert_eq!(Period::parse("d"), Some(Period::Day));
        assert_eq!(Period::parse("daily"), Some(Period::Day));
        assert_eq!(Period::parse("5m"), Some(Period::Min5));
        assert_eq!(Period::parse("m5"), Some(Period::Min5));
        assert_eq!(Period::parse("60m"), Some(Period::Min60));
        assert_eq!(Period::parse("1h"), Some(Period::Min60));
        assert_eq!(Period::parse("xyz"), None);
        // tag() 必须能被 parse() 往返,报错信息里用 tag() 提示才成立。
        for p in [
            Period::Day,
            Period::Min1,
            Period::Min5,
            Period::Min15,
            Period::Min30,
            Period::Min60,
        ] {
            assert_eq!(Period::parse(p.tag()), Some(p));
        }
    }

    #[test]
    fn period_bars_per_year() {
        assert_eq!(Period::Day.bars_per_year(), 252.0);
        // 5分钟:252 交易日 × (240/5) 根 = 12096
        assert_eq!(Period::Min5.bars_per_year(), 252.0 * 48.0);
        assert_eq!(Period::Min60.bars_per_year(), 252.0 * 4.0);
        assert!(Period::Min1.bars_per_year() > Period::Day.bars_per_year());
    }

    #[test]
    fn secid_format() {
        assert_eq!(secid("161226", Market::SZ), "0.161226");
        assert_eq!(secid("600519", Market::SH), "1.600519");
    }

    #[test]
    fn secu_code_format() {
        assert_eq!(secu_code("600519", Market::SH), "600519.SH");
        assert_eq!(secu_code("000858", Market::SZ), "000858.SZ");
    }

    #[test]
    fn hk_secid_and_normalize() {
        assert_eq!(secid("00700", Market::HK), "116.00700");
        // A股 6 位
        assert_eq!(
            normalize_code("600519"),
            Some(("600519".into(), Market::SH))
        );
        assert_eq!(
            normalize_code("000858"),
            Some(("000858".into(), Market::SZ))
        );
        // 港股:显式前缀/后缀、5 位、补零
        assert_eq!(
            normalize_code("hk00700"),
            Some(("00700".into(), Market::HK))
        );
        assert_eq!(
            normalize_code("00700.HK"),
            Some(("00700".into(), Market::HK))
        );
        assert_eq!(normalize_code("00700"), Some(("00700".into(), Market::HK)));
        assert_eq!(normalize_code("700"), Some(("00700".into(), Market::HK))); // 补零到5位
                                                                               // 6 位深市不会被误判成港股
        assert_eq!(
            normalize_code("000700"),
            Some(("000700".into(), Market::SZ))
        );
        assert!(normalize_code("abc").is_none());
    }
}
