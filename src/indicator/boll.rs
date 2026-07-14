use super::ma::sma;

pub struct Boll {
    pub upper: Vec<Option<f64>>,
    pub mid: Vec<Option<f64>>,
    pub lower: Vec<Option<f64>>,
}

/// 布林带：中轨 = SMA(period)，上下轨 = 中轨 ± mult * 标准差（总体标准差）。
pub fn boll(data: &[f64], period: usize, mult: f64) -> Boll {
    let mid = sma(data, period);
    let n = data.len();
    let mut upper = vec![None; n];
    let mut lower = vec![None; n];
    for i in 0..n {
        if let Some(m) = mid[i] {
            let window = &data[i + 1 - period..=i];
            let var = window.iter().map(|x| (x - m).powi(2)).sum::<f64>() / period as f64;
            let sd = var.sqrt();
            upper[i] = Some(m + mult * sd);
            lower[i] = Some(m - mult * sd);
        }
    }
    Boll { upper, mid, lower }
}
