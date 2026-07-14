//! 极简日期工具,只依赖 std,替代 chrono(省去 chrono + iana-time-zone + num-traits)。
//! A 股按北京时间(UTC+8)计,日期用 "YYYY-MM-DD"。

use std::time::{SystemTime, UNIX_EPOCH};

/// 今天(北京时间)的 "YYYY-MM-DD"。
pub fn today() -> String {
    let (y, m, d) = civil_from_days(today_days());
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// 从 date("YYYY-MM-DD")到今天的自然日天数;date 非法时返回 None。
pub fn days_since(date: &str) -> Option<i64> {
    Some(today_days() - parse_ymd(date)?)
}

/// 今天(北京时间)距 1970-01-01 的天数。
fn today_days() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    (secs + 8 * 3600) / 86_400 // UTC+8
}

/// 解析 "YYYY-MM-DD" 为距 1970-01-01 的天数。
pub fn parse_ymd(s: &str) -> Option<i64> {
    let mut it = s.splitn(3, '-');
    let y: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let d: i64 = it.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// 公历日期 → 距 1970-01-01 的天数(Howard Hinnant 算法,public domain)。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// 距 1970-01-01 的天数 → 公历日期。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_known_dates() {
        // 1970-01-01 是第 0 天
        assert_eq!(parse_ymd("1970-01-01"), Some(0));
        // 2000-01-01 距 epoch 10957 天
        assert_eq!(parse_ymd("2000-01-01"), Some(10957));
        // 往返一致
        for &(y, m, d) in &[(2024, 2, 29), (1999, 12, 31), (2025, 7, 14)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
    }

    #[test]
    fn days_between_dates() {
        let a = parse_ymd("2024-01-01").unwrap();
        let b = parse_ymd("2024-12-31").unwrap();
        assert_eq!(b - a, 365); // 2024 是闰年
    }

    #[test]
    fn bad_input_is_none() {
        assert!(parse_ymd("2024-13-01").is_none());
        assert!(parse_ymd("not-a-date").is_none());
        assert!(parse_ymd("2024-01").is_none());
    }
}
