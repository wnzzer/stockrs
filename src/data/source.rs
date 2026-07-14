use std::future::Future;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use super::models::{KLine, Market, Quote};
use super::{eastmoney::Eastmoney, sina::Sina, tencent::Tencent};

/// 带超时的共享 HTTP 客户端。超时是故障切换能生效的前提：
/// 某个源卡住时必须尽快失败，才能切到下一个源。
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (stockrs)")
        .timeout(Duration::from_secs(10))
        .build()
        .context("构建 HTTP 客户端失败")
}

/// 实时行情数据源。
#[async_trait]
pub trait QuoteSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn quote(&self, code: &str, market: Market) -> Result<Quote>;

    /// 批量行情。默认逐个请求；支持批量接口的源应覆盖此方法，
    /// 把 N 个请求压成 1 个（治本，几乎不会被限流）。
    async fn quotes(&self, reqs: &[(String, Market)]) -> Result<Vec<Quote>> {
        let mut out = Vec::with_capacity(reqs.len());
        for (code, market) in reqs {
            out.push(self.quote(code, *market).await?);
        }
        Ok(out)
    }
}

/// 日K线数据源。beg/end 为 "YYYYMMDD"，"0" 表示不限。
#[async_trait]
pub trait KlineSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn klines(
        &self,
        code: &str,
        market: Market,
        beg: &str,
        end: &str,
    ) -> Result<(String, Vec<KLine>)>;
}

/// 故障切换顺序：东财（字段最全）→ 腾讯（前复权+区间）→ 新浪。
fn quote_sources() -> Vec<Box<dyn QuoteSource>> {
    vec![Box::new(Eastmoney), Box::new(Tencent), Box::new(Sina)]
}

fn kline_sources() -> Vec<Box<dyn KlineSource>> {
    vec![Box::new(Eastmoney), Box::new(Tencent), Box::new(Sina)]
}

/// 依次尝试各日K源，返回首个成功结果及其来源名。
pub async fn fetch_klines(
    code: &str,
    market: Market,
    beg: &str,
    end: &str,
) -> Result<(String, Vec<KLine>, &'static str)> {
    let mut errs = Vec::new();
    for src in kline_sources() {
        match src.klines(code, market, beg, end).await {
            Ok((name, ks)) if !ks.is_empty() => return Ok((name, ks, src.name())),
            Ok(_) => errs.push(format!("  [{}] 返回空数据", src.name())),
            Err(e) => errs.push(format!("  [{}] {}", src.name(), e)),
        }
    }
    Err(anyhow!("所有日K源均失败：\n{}", errs.join("\n")))
}

/// 批量行情，带故障切换：优先用批量接口一次拿多只；某源缺失的代码
/// 交给下一个源补齐。返回 (成功的行情+来源, 全源都拿不到的代码)。
pub async fn fetch_quotes(
    reqs: &[(String, Market)],
) -> Result<(Vec<(Quote, &'static str)>, Vec<String>)> {
    let mut found: std::collections::HashMap<String, (Quote, &'static str)> =
        std::collections::HashMap::new();
    let mut remaining: Vec<(String, Market)> = reqs.to_vec();

    for src in quote_sources() {
        if remaining.is_empty() {
            break;
        }
        if let Ok(quotes) = src.quotes(&remaining).await {
            for q in quotes {
                found.entry(q.code.clone()).or_insert((q, src.name()));
            }
            remaining.retain(|(c, _)| !found.contains_key(c));
        }
    }

    let ordered = reqs
        .iter()
        .filter_map(|(c, _)| found.remove(c))
        .collect::<Vec<_>>();
    let failed = remaining.into_iter().map(|(c, _)| c).collect();
    Ok((ordered, failed))
}

/// 指数退避 + 抖动重试。抖动打散并发请求的发出时刻，避免整点齐射。
pub async fn with_retry<T, F, Fut>(tries: u32, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt >= tries {
                    return Err(e);
                }
                // 300ms, 600ms, 1200ms... 各加 0~300ms 抖动
                let backoff = 300u64 * (1 << (attempt - 1)) + jitter_ms(300);
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }
        }
    }
}

/// 无依赖的伪随机抖动，取系统时间纳秒对 span 取模。
pub fn jitter_ms(span: u64) -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % span.max(1))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 联网烟雾测试，默认忽略：cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn tencent_quote_live() {
        let q = Tencent.quote("600519", Market::SH).await.unwrap();
        assert!(q.price > 0.0 && q.high >= q.low);
    }

    #[tokio::test]
    #[ignore]
    async fn sina_quote_live() {
        let q = Sina.quote("600519", Market::SH).await.unwrap();
        assert!(q.price > 0.0 && q.high >= q.low);
    }

    #[tokio::test]
    #[ignore]
    async fn tencent_kline_live() {
        let (_, ks) = Tencent
            .klines("600519", Market::SH, "20240101", "20240201")
            .await
            .unwrap();
        assert!(!ks.is_empty());
        assert!(ks[0].close > 0.0);
    }

    #[tokio::test]
    #[ignore]
    async fn sina_kline_live() {
        let (_, ks) = Sina.klines("600519", Market::SH, "0", "0").await.unwrap();
        assert!(!ks.is_empty());
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn eastmoney_batch_live() {
        let q = Eastmoney
            .quotes(&[("600519".into(), Market::SH), ("000858".into(), Market::SZ)])
            .await
            .unwrap();
        assert_eq!(q.len(), 2);
        assert!(q.iter().all(|x| x.price > 0.0 && x.high >= x.low));
    }

    #[tokio::test]
    #[ignore]
    async fn tencent_batch_live() {
        let q = Tencent
            .quotes(&[("600519".into(), Market::SH), ("000858".into(), Market::SZ)])
            .await
            .unwrap();
        assert_eq!(q.len(), 2);
        assert!(q.iter().all(|x| x.price > 0.0 && x.high >= x.low));
    }

    #[tokio::test]
    #[ignore]
    async fn sina_batch_live() {
        let q = Sina
            .quotes(&[("600519".into(), Market::SH), ("000858".into(), Market::SZ)])
            .await
            .unwrap();
        assert_eq!(q.len(), 2);
        assert!(q.iter().all(|x| x.price > 0.0 && x.high >= x.low));
    }
}
