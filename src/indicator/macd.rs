use super::ma::ema;

pub struct Macd {
    pub dif: Vec<Option<f64>>,
    pub dea: Vec<Option<f64>>,
    pub macd: Vec<Option<f64>>,
}

/// MACD：DIF = EMA(fast) - EMA(slow)，DEA = EMA(DIF, signal)，MACD = 2*(DIF-DEA)。
pub fn macd(data: &[f64], fast: usize, slow: usize, signal: usize) -> Macd {
    let ema_fast = ema(data, fast);
    let ema_slow = ema(data, slow);
    let dif: Vec<Option<f64>> = ema_fast
        .iter()
        .zip(&ema_slow)
        .map(|(f, s)| match (f, s) {
            (Some(f), Some(s)) => Some(f - s),
            _ => None,
        })
        .collect();

    // 对 DIF 的有效段做 EMA
    let dif_vals: Vec<f64> = dif.iter().filter_map(|v| *v).collect();
    let dea_partial = ema(&dif_vals, signal);
    let mut dea = vec![None; data.len()];
    let mut j = 0;
    for i in 0..data.len() {
        if dif[i].is_some() {
            dea[i] = dea_partial[j];
            j += 1;
        }
    }

    let macd_line: Vec<Option<f64>> = dif
        .iter()
        .zip(&dea)
        .map(|(d, e)| match (d, e) {
            (Some(d), Some(e)) => Some(2.0 * (d - e)),
            _ => None,
        })
        .collect();

    Macd {
        dif,
        dea,
        macd: macd_line,
    }
}
