// 量价突破 —— 分钟线量能策略示例
//
//   stockrs data update --period 5m 600519
//   stockrs backtest strategies/vol_breakout.rhai --stock 600519 --period 5m --capital 200000
//
// 逻辑:放量且上涨则买入(量比确认承接),缩量则离场。量比 = 当前量 / 过去 N 根均量,
// >1 放量、<1 缩量;只看绝对成交量无法区分,必须对比历史,故用 ctx.volume_ratio。

let name = "Vol Breakout";
let description = "放量上涨买入,缩量离场(量价配合)";

fn on_bar(ctx) {
    let vr = ctx.volume_ratio(20);      // 相对过去 20 根的量比
    if vr != vr { return; }             // 数据不足(NaN)先跳过

    // 放量 + 较上一根收高 -> 视为有承接的突破,满仓买入
    if ctx.position() == 0 && vr > 1.5 && ctx.close > ctx.close_at(1) {
        ctx.buy(ctx.close, ctx.max_shares());
    }

    // 缩量 -> 承接减弱,清仓离场
    if ctx.position() > 0 && vr < 0.7 {
        ctx.sell(ctx.close, ctx.position());
    }
}
