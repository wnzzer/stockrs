/// 简单移动平均，返回与输入等长的序列，不足周期处为 None。
pub fn sma(data: &[f64], period: usize) -> Vec<Option<f64>> {
    if period == 0 {
        return vec![None; data.len()];
    }
    let mut out = vec![None; data.len()];
    let mut sum = 0.0;
    for i in 0..data.len() {
        sum += data[i];
        if i >= period {
            sum -= data[i - period];
        }
        if i + 1 >= period {
            out[i] = Some(sum / period as f64);
        }
    }
    out
}

/// 指数移动平均。首个有效值用前 period 个的 SMA 作为种子。
pub fn ema(data: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; data.len()];
    if period == 0 || data.len() < period {
        return out;
    }
    let k = 2.0 / (period as f64 + 1.0);
    let seed = data[..period].iter().sum::<f64>() / period as f64;
    let mut prev = seed;
    out[period - 1] = Some(seed);
    for i in period..data.len() {
        prev = data[i] * k + prev * (1.0 - k);
        out[i] = Some(prev);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sma_basic() {
        let d = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let r = sma(&d, 3);
        assert_eq!(r[0], None);
        assert_eq!(r[1], None);
        assert_eq!(r[2], Some(2.0));
        assert_eq!(r[4], Some(4.0));
    }

    #[test]
    fn ema_seed_is_sma() {
        let d = vec![1.0, 2.0, 3.0, 4.0];
        let r = ema(&d, 2);
        assert_eq!(r[0], None);
        assert_eq!(r[1], Some(1.5));
        assert!(r[3].unwrap() > r[1].unwrap());
    }
}
