// ===========================================================================
//  stockrs 策略模板 —— 由 `stockrs strategy new` 生成
//
//  策略是一个 .rhai 脚本,定义 on_bar(ctx),单标的回测下每根日K调用一次。
//  信号在当日收盘计算、次日开盘成交(无未来函数)。改掉下面的 on_bar 即可。
//
//  运行:
//    stockrs backtest 本文件 --stock 600519 --start 2023-01-01 --end 2025-01-01
//    stockrs backtest 本文件 --stock hk00700                       # 港股同样适用
//    stockrs backtest 本文件 --stock 600519 --param fast=5,10 --optimize sharpe
// ===========================================================================
//
//  ctx API 速查(单标的):
//
//    行情(属性)    ctx.open  ctx.high  ctx.low  ctx.close  ctx.volume  ctx.date
//    历史          ctx.close_at(n)                   n 天前收盘价
//    指标          ctx.sma(p)   ctx.ema(p)   ctx.rsi(p)
//                  ctx.sma_at(p, n)                  n 天前的 SMA
//                  ctx.macd(fast, slow, signal)      返回 [dif, dea, macd]
//                  ctx.kdj(p)                        返回 [k, d, j]
//                  ctx.boll(p, mult)                 返回 [upper, mid, lower]
//    基本面(属性)  ctx.pe  ctx.pb  ctx.ps  ctx.mktcap 按 bar 无未来函数对齐,无数据为 NaN
//    账户          ctx.position()  ctx.cash()  ctx.total_value()  ctx.max_shares()
//    交易          ctx.buy(price, shares)   ctx.sell(price, shares)
//    参数          ctx.param(name, default)          --param 注入;默认整数则返回整数,小数则小数
//    状态(跨 bar)  ctx.set(key, v)  ctx.get(key, default)  ctx.has(key)
//
//  两个坑:
//    1) 指标数据不足 / 基本面缺失时返回 NaN。判 NaN 用 x != x(NaN 不等于自身)。
//    2) Rhai 区分整数与浮点:周期类参数写整数(5、20),比例/乘数类写小数(0.5、2.0),
//       否则类型不匹配报错。
// ===========================================================================

let name = "My Strategy";
let description = "在这里描述你的策略";

fn on_bar(ctx) {
    // —— 示例:5/20 双均线金叉买入、死叉卖出。按需替换成你自己的逻辑。 ——
    let ma5 = ctx.sma(5);
    let ma20 = ctx.sma(20);
    let prev5 = ctx.sma_at(5, 1);
    let prev20 = ctx.sma_at(20, 1);

    // 数据不足(NaN)先跳过,避免误判
    if ma5 != ma5 || ma20 != ma20 || prev5 != prev5 || prev20 != prev20 {
        return;
    }

    // 金叉且空仓 -> 满仓买入
    if prev5 < prev20 && ma5 > ma20 && ctx.position() == 0 {
        ctx.buy(ctx.close, ctx.max_shares());
    }

    // 死叉且有持仓 -> 清仓
    if prev5 > prev20 && ma5 < ma20 && ctx.position() > 0 {
        ctx.sell(ctx.close, ctx.position());
    }
}
