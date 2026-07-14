// 双均线 + 止盈止损(演示策略状态 ctx.set/get)
//
// 金叉买入并用 ctx.set 记录建仓价;持仓中若死叉 / 跌破建仓价 10%(止损)/
// 涨过建仓价 20%(止盈)则卖出。Rhai 函数访问不了脚本全局变量,状态挂 ctx 上最省事。

let name = "SMA Cross + Stop";
let description = "金叉买入记录建仓价,死叉/止损-10%/止盈+20% 卖出";

fn on_bar(ctx) {
    let ma5 = ctx.sma(5);
    let ma20 = ctx.sma(20);
    let prev5 = ctx.sma_at(5, 1);
    let prev20 = ctx.sma_at(20, 1);
    if ma5 != ma5 || ma20 != ma20 || prev5 != prev5 || prev20 != prev20 {
        return;
    }

    let price = ctx.close;

    if ctx.position() == 0 {
        // 金叉买入(次日开盘成交),记录建仓价(近似:用信号日收盘)
        if prev5 < prev20 && ma5 > ma20 {
            ctx.buy(price, ctx.max_shares());
            ctx.set("entry", price);
        }
        return;
    }

    // 持仓中:死叉 / 止损 / 止盈
    let entry = ctx.get("entry", 0.0);
    let dead_cross = prev5 > prev20 && ma5 < ma20;
    let stop_loss = entry > 0.0 && price < entry * 0.90;
    let take_profit = entry > 0.0 && price > entry * 1.20;
    if dead_cross || stop_loss || take_profit {
        ctx.sell(price, ctx.position());
    }
}
