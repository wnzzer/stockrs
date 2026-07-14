// 超跌反弹：RSI超卖 + 布林下轨买入，RSI超买卖出
//
//   stockrs backtest strategies/oversold_boll.rhai --stock 601677 \
//     --param rsi_buy=25,30,35 --param rsi_sell=60,70,80 --optimize sharpe

let name = "Oversold + Bollinger";
let description = "RSI超卖+布林下轨买入，RSI回升卖出";

fn on_bar(ctx) {
    let rsi_buy = ctx.param("rsi_buy", 30.0);
    let rsi_sell = ctx.param("rsi_sell", 70.0);

    let rsi = ctx.rsi(6);
    if rsi != rsi { return; }

    let boll = ctx.boll(20, 2.0);
    let lower = boll[2];
    if lower != lower { return; }

    let price = ctx.close;
    let pos = ctx.position();

    if pos == 0 {
        let limit = lower * 1.03;
        if rsi < rsi_buy {
            if price <= limit {
                let s = ctx.max_shares();
                if s > 0 {
                    ctx.buy(price, s);
                }
            }
        }
        return;
    }

    if rsi > rsi_sell {
        ctx.sell(price, pos);
    }
}
