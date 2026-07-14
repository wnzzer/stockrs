// 动量轮动组合策略（组合回测：--stocks 或 --universe）
//
// 每个交易日按 lookback 日涨幅给活跃股票排名，等权持有前 top_n 名，
// 其余清仓。展示横截面选股(ctx.rank) + 组合再平衡(ctx.order_target_pct)。
//
// 可扫描参数：--param top_n=1,2,3 --param lookback=10,20,60

let name = "Momentum Rotation";
let description = "按动量排名等权持有前 N 名，每日再平衡";

fn on_bar(ctx) {
    let top_n = ctx.param("top_n", 2);
    let lookback = ctx.param("lookback", 20);

    let ranked = ctx.rank(lookback);   // 动量降序的活跃股票代码
    if ranked.len() < top_n {
        return;
    }

    // 选中前 top_n 名
    let selected = [];
    for i in 0..top_n {
        selected.push(ranked[i]);
    }

    // 清掉不在选中列表里的持仓
    for code in ctx.stocks() {
        if ctx.position(code) > 0 && !selected.contains(code) {
            ctx.order_target_pct(code, 0.0);
        }
    }

    // 选中的等权买入（留 2% 现金缓冲，避免开盘跳空导致现金不足）
    let weight = 0.98 / top_n.to_float();
    for code in selected {
        ctx.order_target_pct(code, weight);
    }
}
