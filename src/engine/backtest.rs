use anyhow::Result;

use super::context::{Ctx, Inner, Order, TradeRec};
use super::metrics::{self, Metrics};
use super::rules::floor_to_lot;

pub struct BacktestResult {
    pub metrics: Metrics,
    pub trades: Vec<TradeRec>,
}

/// 驱动回测主循环。每根 bar：先按开盘价撮合上一根挂出的订单，再调用 on_bar。
pub fn run<F>(ctx: &Ctx, mut on_bar: F) -> Result<BacktestResult>
where
    F: FnMut() -> Result<()>,
{
    let n = ctx.0.lock().unwrap().klines.len();
    let initial = ctx.0.lock().unwrap().cash;
    let bars_per_year = ctx.0.lock().unwrap().period.bars_per_year();

    for i in 0..n {
        {
            let mut s = ctx.0.lock().unwrap();
            s.i = i;
            let order = s.pending.take();
            if let Some(order) = order {
                fill(&mut s, order);
            }
            let equity = s.cash + s.position as f64 * s.klines[i].close;
            s.equity.push(equity);
        }
        on_bar()?;
    }

    let s = ctx.0.lock().unwrap();
    let metrics = metrics::compute(initial, &s.equity, &s.trades, bars_per_year);
    Ok(BacktestResult {
        metrics,
        trades: s.trades.clone(),
    })
}

fn fill(s: &mut Inner, order: Order) {
    let price = s.klines[s.i].open;
    let date = s.klines[s.i].date.clone();
    if price <= 0.0 {
        return;
    }

    if order.buy {
        let mut shares = order.shares;
        // 现金不足时下调到可负担的整手数
        let unit_cost = price * (1.0 + s.fee.buy_rate_approx());
        let affordable = floor_to_lot((s.cash / unit_cost) as i64, s.lot);
        shares = shares.min(affordable);
        if shares <= 0 {
            return;
        }
        let cost = shares as f64 * price;
        let fee = s.fee.buy_cost(cost);
        let prev_cost = s.avg_cost * s.position as f64;
        s.position += shares;
        s.avg_cost = (prev_cost + cost) / s.position as f64;
        s.cash -= cost + fee;
        s.trades.push(TradeRec {
            date,
            action: "BUY".to_string(),
            price,
            shares,
            pnl_pct: None,
        });
    } else {
        let shares = order.shares.min(s.position);
        if shares <= 0 {
            return;
        }
        let proceeds = shares as f64 * price;
        let fee = s.fee.sell_cost(proceeds);
        let pnl_pct = if s.avg_cost > 0.0 {
            Some((price - s.avg_cost) / s.avg_cost)
        } else {
            None
        };
        s.cash += proceeds - fee;
        s.position -= shares;
        if s.position == 0 {
            s.avg_cost = 0.0;
        }
        s.trades.push(TradeRec {
            date,
            action: "SELL".to_string(),
            price,
            shares,
            pnl_pct,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::KLine;

    fn k(date: &str, o: f64, c: f64) -> KLine {
        KLine {
            code: "T".into(),
            date: date.into(),
            open: o,
            high: c.max(o),
            low: c.min(o),
            close: c,
            volume: 0.0,
            amount: 0.0,
            turnover: None,
        }
    }

    #[test]
    fn buy_fills_next_open_and_realizes_pnl() {
        let klines = vec![
            k("d1", 10.0, 10.0),
            k("d2", 10.0, 10.0),
            k("d3", 12.0, 12.0),
        ];
        let ctx = Ctx::new(
            klines,
            100_000.0,
            &[],
            100,
            crate::engine::rules::FeeModel::a_share(),
            crate::data::Period::Day,
        );
        let mut bar = 0;
        let c = ctx.clone();
        let res = run(&ctx, move || {
            // d1 下买单 -> d2 开盘 10 成交；d2 下卖单 -> d3 开盘 12 成交
            if bar == 0 {
                c.clone().buy(10.0, 100);
            } else if bar == 1 {
                c.clone().sell(10.0, 100);
            }
            bar += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(res.trades.len(), 2);
        assert_eq!(res.trades[0].action, "BUY");
        assert_eq!(res.trades[1].action, "SELL");
        // 卖出价 12 相对成本 10，约 +20%
        let pnl = res.trades[1].pnl_pct.unwrap();
        assert!((pnl - 0.2).abs() < 1e-6);
        assert!(res.metrics.final_value > 100_000.0);
    }
}
