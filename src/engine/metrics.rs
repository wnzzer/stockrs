use super::context::TradeRec;

/// 基准对比结果。独立于 Metrics，回测提供 --benchmark 时才计算。
pub struct Benchmark {
    pub name: String,
    pub code: String,
    /// 基准同期买入持有收益
    pub total_return: f64,
    /// 超额收益 = 策略收益 - 基准收益
    pub excess: f64,
    /// 相对基准的 Beta；样本不足时为 NaN
    pub beta: f64,
    /// 年化 Alpha（日 alpha × 252）；样本不足时为 NaN
    pub alpha_annual: f64,
}

/// 由资金曲线求逐日收益。保持与输入等长（首日无收益，从第二点起），
/// 不跳过任何点以保证策略/基准逐日对齐（Beta 依赖对齐）。
fn daily_returns(equity: &[f64]) -> Vec<f64> {
    equity
        .windows(2)
        .map(|w| if w[0] != 0.0 { w[1] / w[0] - 1.0 } else { 0.0 })
        .collect()
}

/// 给定对齐到同一日期轴、且已归一到同一初始资金的策略与基准资金曲线，
/// 计算基准收益、超额、Beta、年化 Alpha。两条曲线应等长且逐日对齐。
pub fn compute_benchmark(
    name: String,
    code: String,
    strat_equity: &[f64],
    bench_equity: &[f64],
) -> Benchmark {
    let n = strat_equity.len().min(bench_equity.len());
    let strat_return = ret_over(&strat_equity[..n]);
    let total_return = ret_over(&bench_equity[..n]);

    let rs = daily_returns(&strat_equity[..n]);
    let rb = daily_returns(&bench_equity[..n]);
    let m = rs.len().min(rb.len());

    // 少于 3 个日收益样本时 cov/var 统计意义太弱，Beta 极不稳定，直接降级为 NaN。
    let (beta, alpha_annual) = if m >= 3 {
        let ms = rs[..m].iter().sum::<f64>() / m as f64;
        let mb = rb[..m].iter().sum::<f64>() / m as f64;
        let mut cov = 0.0;
        let mut var_b = 0.0;
        for i in 0..m {
            cov += (rs[i] - ms) * (rb[i] - mb);
            var_b += (rb[i] - mb).powi(2);
        }
        if var_b > 0.0 {
            let beta = cov / var_b;
            let alpha_daily = ms - beta * mb;
            (beta, alpha_daily * 252.0)
        } else {
            (f64::NAN, f64::NAN)
        }
    } else {
        (f64::NAN, f64::NAN)
    };

    Benchmark {
        name,
        code,
        total_return,
        excess: strat_return - total_return,
        beta,
        alpha_annual,
    }
}

fn ret_over(equity: &[f64]) -> f64 {
    match (equity.first(), equity.last()) {
        (Some(&a), Some(&b)) if a != 0.0 => b / a - 1.0,
        _ => 0.0,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_beta_and_alpha() {
        // 策略逐日收益恰为基准的 2 倍 => Beta≈2, Alpha≈0
        let bench = vec![100.0, 110.0, 104.5, 106.59]; // 收益 +0.10,-0.05,+0.02
        let strat = vec![100.0, 120.0, 108.0, 112.32]; // 收益 +0.20,-0.10,+0.04
        let b = compute_benchmark("HS300".into(), "000300".into(), &strat, &bench);
        assert!((b.beta - 2.0).abs() < 1e-6, "beta={}", b.beta);
        assert!(b.alpha_annual.abs() < 1e-6, "alpha={}", b.alpha_annual);
        assert!((b.total_return - (106.59 / 100.0 - 1.0)).abs() < 1e-9);
        assert!(b.excess > 0.0);
    }

    #[test]
    fn benchmark_insufficient_samples() {
        let b = compute_benchmark("X".into(), "0".into(), &[100.0], &[100.0]);
        assert!(b.beta.is_nan());
        assert!(b.alpha_annual.is_nan());
    }
}
