/// 单个持仓从建仓日到最新的浮动盈亏分析（纯计算，便于单测）。
pub struct PositionStats {
    pub trading_days: usize,
    pub qty: i64,
    pub avg_cost: f64,
    pub cost: f64,
    pub last_close: f64,
    pub value: f64,
    pub pnl: f64,
    pub pnl_pct: f64,
    /// 每交易日平均浮盈（元）
    pub avg_daily_pnl: f64,
    pub ret_day: Option<f64>,
    pub ret_week: Option<f64>,
    pub ret_month: Option<f64>,
    /// (浮盈额, 浮盈%, 日期)
    pub max_profit: (f64, f64, String),
    pub max_loss: (f64, f64, String),
    /// 持仓期间市值的最大回撤（peak→trough，负数）
    pub max_drawdown: f64,
    /// 每日浮盈% 序列，用于画曲线
    pub pnl_pct_series: Vec<f64>,
}

/// dates/closes 为建仓日起的每个交易日收盘（升序，等长且非空）。
pub fn compute(avg_cost: f64, qty: i64, dates: &[String], closes: &[f64]) -> PositionStats {
    let n = closes.len();
    let cost = avg_cost * qty as f64;
    let qf = qty as f64;

    let value_series: Vec<f64> = closes.iter().map(|c| c * qf).collect();
    let pnl_series: Vec<f64> = value_series.iter().map(|v| v - cost).collect();
    let pnl_pct_series: Vec<f64> = pnl_series
        .iter()
        .map(|p| if cost != 0.0 { p / cost } else { 0.0 })
        .collect();

    let last_close = closes[n - 1];
    let value = value_series[n - 1];
    let pnl = pnl_series[n - 1];
    let pnl_pct = pnl_pct_series[n - 1];

    let ret_over = |k: usize| {
        if n > k && closes[n - 1 - k] != 0.0 {
            Some(last_close / closes[n - 1 - k] - 1.0)
        } else {
            None
        }
    };

    // 最大浮盈 / 最大浮亏（相对成本的极值点）
    let mut max_profit = (pnl_series[0], pnl_pct_series[0], dates[0].clone());
    let mut max_loss = (pnl_series[0], pnl_pct_series[0], dates[0].clone());
    for i in 0..n {
        if pnl_series[i] > max_profit.0 {
            max_profit = (pnl_series[i], pnl_pct_series[i], dates[i].clone());
        }
        if pnl_series[i] < max_loss.0 {
            max_loss = (pnl_series[i], pnl_pct_series[i], dates[i].clone());
        }
    }

    // 持仓回撤：市值相对历史峰值的最大跌幅
    let mut peak = f64::MIN;
    let mut max_drawdown = 0.0;
    for &v in &value_series {
        if v > peak {
            peak = v;
        }
        if peak > 0.0 {
            let dd = (v - peak) / peak;
            if dd < max_drawdown {
                max_drawdown = dd;
            }
        }
    }

    PositionStats {
        trading_days: n,
        qty,
        avg_cost,
        cost,
        last_close,
        value,
        pnl,
        pnl_pct,
        avg_daily_pnl: pnl / n as f64,
        ret_day: ret_over(1),
        ret_week: ret_over(5),
        ret_month: ret_over(20),
        max_profit,
        max_loss,
        max_drawdown,
        pnl_pct_series,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_stats() {
        // 成本 10，持 100 股，收盘 10→12→9→11
        let dates: Vec<String> = ["d1", "d2", "d3", "d4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let closes = vec![10.0, 12.0, 9.0, 11.0];
        let s = compute(10.0, 100, &dates, &closes);
        assert_eq!(s.trading_days, 4);
        assert!((s.pnl - 100.0).abs() < 1e-6); // (11-10)*100
        assert!((s.pnl_pct - 0.1).abs() < 1e-6);
        assert_eq!(s.max_profit.2, "d2"); // 峰值在 12
        assert_eq!(s.max_loss.2, "d3"); //  谷值在 9
        assert!((s.max_drawdown - (-0.25)).abs() < 1e-6); // 1200→900
        assert!((s.ret_day.unwrap() - (11.0 / 9.0 - 1.0)).abs() < 1e-6);
    }
}
