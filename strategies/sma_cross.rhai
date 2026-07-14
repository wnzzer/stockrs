let name = "SMA Cross";
let description = "5日均线上穿20日均线买入，下穿卖出";

fn on_bar(ctx) {
    let ma5 = ctx.sma(5);
    let ma20 = ctx.sma(20);
    let prev_ma5 = ctx.sma_at(5, 1);
    let prev_ma20 = ctx.sma_at(20, 1);

    if prev_ma5 < prev_ma20 && ma5 > ma20 {
        ctx.buy(ctx.close, ctx.max_shares());
    }

    if prev_ma5 > prev_ma20 && ma5 < ma20 {
        ctx.sell(ctx.close, ctx.position());
    }
}
