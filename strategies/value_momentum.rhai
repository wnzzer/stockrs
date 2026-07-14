// 双重验证(组合):在低估值股票池里按动量轮动
//
// 每日按动量排名,从中筛出 PB 低于阈值(低估)的,等权持有前 top_n 名。
// 展示组合 ctx.pb(code) 基本面接口 + 横截面选股 + 再平衡的结合。
//
//   stockrs backtest strategies/value_momentum.rhai --universe \
//     --param top_n=2,3 --param pb_max=3,5 --optimize sharpe

let name = "Value Momentum";
let description = "低 PB 股票池里按动量轮动前 N 名";

fn on_bar(ctx) {
    let top_n = ctx.param("top_n", 2);
    let lookback = ctx.param("lookback", 20);
    let pb_max = ctx.param("pb_max", 5.0);

    let ranked = ctx.rank(lookback); // 活跃股按动量降序

    // 从动量榜里过滤出低估值(PB 有效且低于阈值)的,取前 top_n
    let selected = [];
    for code in ranked {
        if selected.len() >= top_n {
            break;
        }
        let pb = ctx.pb(code);
        if pb == pb && pb > 0.0 && pb < pb_max {
            selected.push(code);
        }
    }

    // 清掉不在选中列表里的持仓
    for code in ctx.stocks() {
        if ctx.position(code) > 0 && !selected.contains(code) {
            ctx.order_target_pct(code, 0.0);
        }
    }

    // 选中的等权买入(留 2% 现金缓冲)
    if selected.len() > 0 {
        let weight = 0.98 / selected.len().to_float();
        for code in selected {
            ctx.order_target_pct(code, weight);
        }
    }
}
