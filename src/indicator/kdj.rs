pub struct Kdj {
    pub k: Vec<Option<f64>>,
    pub d: Vec<Option<f64>>,
    pub j: Vec<Option<f64>>,
}

/// KDJ：RSV 周期 period，K/D 用 1/3 平滑（初值 50）。
pub fn kdj(high: &[f64], low: &[f64], close: &[f64], period: usize) -> Kdj {
    let n = close.len();
    let mut k = vec![None; n];
    let mut d = vec![None; n];
    let mut j = vec![None; n];
    if period == 0 || n < period {
        return Kdj { k, d, j };
    }
    let mut prev_k = 50.0;
    let mut prev_d = 50.0;
    for i in (period - 1)..n {
        let hh = high[i + 1 - period..=i]
            .iter()
            .cloned()
            .fold(f64::MIN, f64::max);
        let ll = low[i + 1 - period..=i]
            .iter()
            .cloned()
            .fold(f64::MAX, f64::min);
        let rsv = if hh - ll == 0.0 {
            0.0
        } else {
            (close[i] - ll) / (hh - ll) * 100.0
        };
        let cur_k = 2.0 / 3.0 * prev_k + 1.0 / 3.0 * rsv;
        let cur_d = 2.0 / 3.0 * prev_d + 1.0 / 3.0 * cur_k;
        k[i] = Some(cur_k);
        d[i] = Some(cur_d);
        j[i] = Some(3.0 * cur_k - 2.0 * cur_d);
        prev_k = cur_k;
        prev_d = cur_d;
    }
    Kdj { k, d, j }
}
