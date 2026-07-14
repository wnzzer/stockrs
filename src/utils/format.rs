/// 金额千分位格式化，如 100000.0 -> "100,000.00"。
pub fn money(v: f64) -> String {
    let sign = if v < 0.0 { "-" } else { "" };
    let v = v.abs();
    let int_part = v.trunc() as i64;
    let frac = ((v - int_part as f64) * 100.0).round() as i64;
    format!("{}{}.{:02}", sign, thousands(int_part), frac)
}

/// 字符的终端显示宽度：CJK / 全角字符占 2 列，其余占 1 列。
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F   // Hangul Jamo
        | 0x2E80..=0x303E // CJK 部首 / 标点
        | 0x3041..=0x33FF // 平假名/片假名/CJK 符号
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 统一表意
        | 0xA000..=0xA4CF // 彝文
        | 0xAC00..=0xD7A3 // 谚文音节
        | 0xF900..=0xFAFF // CJK 兼容表意
        | 0xFE30..=0xFE4F // CJK 兼容形式
        | 0xFF00..=0xFF60 // 全角 ASCII / 标点
        | 0xFFE0..=0xFFE6 // 全角符号
        | 0x1F300..=0x1FAFF // Emoji
        | 0x20000..=0x3FFFD // CJK 扩展 B+
    );
    if wide {
        2
    } else {
        1
    }
}

/// 字符串的显示宽度。
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按显示宽度右侧补空格到 width 列（超出则原样返回）。
pub fn pad_end(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

fn thousands(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let bytes = s.as_bytes();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
