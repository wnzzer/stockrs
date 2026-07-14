use super::context::TradeRec;

pub struct Metrics {
    pub initial: f64,
    pub final_value: f64,
    pub total_return: f64,
    pub annual_return: f64,
    pub max_drawdown: f64,
    pub sharpe: f64,
    pub win_rate: f64,
    pub wins: usize,
    pub closed_trades: usize,
    pub total_trades: usize,
}

pub fn compute(initial: f64, equity: &[f64], trades: &[TradeRec]) -> Metrics {
    let final_value = equity.last().copied().unwrap_or(initial);
    let total_return = if initial != 0.0 {
        final_value / initial - 1.0
    } else {
        0.0
    };

    let days = equity.len().max(1);
    let years = days as f64 / 252.0;
    let annual_return = if years > 0.0 && final_value > 0.0 && initial > 0.0 {
        (final_value / initial).powf(1.0 / years) - 1.0
    } else {
        total_return
    };

    // 最大回撤
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;
    for &v in equity {
        if v > peak {
            peak = v;
        }
        if peak > 0.0 {
            let dd = (v - peak) / peak;
            if dd < max_dd {
                max_dd = dd;
            }
        }
    }

    // 夏普（日收益，年化，无风险利率取 0）
    let mut rets = Vec::new();
    for w in equity.windows(2) {
        if w[0] != 0.0 {
            rets.push(w[1] / w[0] - 1.0);
        }
    }
    let sharpe = if rets.len() > 1 {
        let mean = rets.iter().sum::<f64>() / rets.len() as f64;
        let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / rets.len() as f64;
        let sd = var.sqrt();
        if sd > 0.0 {
            mean / sd * (252f64).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };

    let closed: Vec<&TradeRec> = trades.iter().filter(|t| t.pnl_pct.is_some()).collect();
    let wins = closed
        .iter()
        .filter(|t| t.pnl_pct.unwrap_or(0.0) > 0.0)
        .count();
    let closed_trades = closed.len();
    let win_rate = if closed_trades > 0 {
        wins as f64 / closed_trades as f64
    } else {
        0.0
    };

    Metrics {
        initial,
        final_value,
        total_return,
        annual_return,
        max_drawdown: max_dd,
        sharpe,
        win_rate,
        wins,
        closed_trades,
        total_trades: trades.len(),
    }
}
