// 参数化双均线（单标的，用于参数扫描示例）
//
// 快线上穿慢线买入，下穿卖出。快慢周期由参数注入，缺省 5 / 20。
// 参数扫描：
//   stockrs backtest strategies/sma_cross_param.rhai --stock 600519 \
//     --param fast=5,10 --param slow=20,30,60 --optimize sharpe

let name = "SMA Cross (param)";
let description = "参数化双均线，可用 --param fast/slow 扫描";

fn on_bar(ctx) {
    let fast = ctx.param("fast", 5);
    let slow = ctx.param("slow", 20);

    let ma_f = ctx.sma(fast);
    let ma_s = ctx.sma(slow);
    let prev_f = ctx.sma_at(fast, 1);
    let prev_s = ctx.sma_at(slow, 1);

    // NaN 保护（数据不足）
    if ma_f != ma_f || ma_s != ma_s || prev_f != prev_f || prev_s != prev_s {
        return;
    }

    if prev_f < prev_s && ma_f > ma_s {
        ctx.buy(ctx.close, ctx.max_shares());
    }
    if prev_f > prev_s && ma_f < ma_s {
        ctx.sell(ctx.close, ctx.position());
    }
}
