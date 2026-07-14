// 双重验证(单标的):技术面金叉 + 基本面低估值才买
//
// 5/20 双均线金叉,且当日 PB 低于阈值(低估)才买入;死叉卖出。
// 展示 ctx.pe / ctx.pb / ctx.ps / ctx.mktcap 无参基本面接口(按 bar 无未来函数对齐)。
//
//   stockrs backtest strategies/value_sma.rhai --stock 600519 \
//     --param pb_max=3,5,8 --optimize sharpe

let name = "Value + SMA Cross";
let description = "金叉且 PB 低于阈值才买,死叉卖出";

fn on_bar(ctx) {
    let pb_max = ctx.param("pb_max", 5.0);

    let ma5 = ctx.sma(5);
    let ma20 = ctx.sma(20);
    let prev5 = ctx.sma_at(5, 1);
    let prev20 = ctx.sma_at(20, 1);
    if ma5 != ma5 || ma20 != ma20 || prev5 != prev5 || prev20 != prev20 {
        return;
    }

    let pb = ctx.pb; // 无参基本面 getter(当日 PB,无数据为 NaN)

    // 金叉 + 低估值(PB 有效且低于阈值)才买
    if prev5 < prev20 && ma5 > ma20 && ctx.position() == 0 {
        if pb == pb && pb > 0.0 && pb < pb_max {
            ctx.buy(ctx.close, ctx.max_shares());
        }
    }

    // 死叉卖出
    if prev5 > prev20 && ma5 < ma20 && ctx.position() > 0 {
        ctx.sell(ctx.close, ctx.position());
    }
}
