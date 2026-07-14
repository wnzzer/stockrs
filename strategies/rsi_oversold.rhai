let name = "RSI Oversold";
let description = "RSI 低于 30 超卖买入，高于 70 超买卖出";

fn on_bar(ctx) {
    let r = ctx.rsi(14);
    if r != r { return; }   // NaN 保护（数据不足）

    if r < 30.0 && ctx.position() == 0 {
        ctx.buy(ctx.close, ctx.max_shares());
    }

    if r > 70.0 && ctx.position() > 0 {
        ctx.sell(ctx.close, ctx.position());
    }
}
