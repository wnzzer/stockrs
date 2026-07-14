use anyhow::{anyhow, Context, Result};
use rhai::{Engine, Scope, AST};

use crate::engine::context::Ctx;

/// 构建注册了 ctx 方法的 Rhai 引擎。
pub fn build_engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_type_with_name::<Ctx>("Ctx");

    // 行情字段（注册为属性 getter，脚本里可写 ctx.close）
    engine.register_get("open", Ctx::open);
    engine.register_get("high", Ctx::high);
    engine.register_get("low", Ctx::low);
    engine.register_get("close", Ctx::close);
    engine.register_get("volume", Ctx::volume);
    engine.register_get("date", Ctx::date);

    // 历史
    engine.register_fn("close_at", Ctx::close_at);

    // 指标
    engine.register_fn("sma", Ctx::sma);
    engine.register_fn("sma_at", Ctx::sma_at);
    engine.register_fn("ema", Ctx::ema);
    engine.register_fn("rsi", Ctx::rsi);
    engine.register_fn("macd", Ctx::macd);
    engine.register_fn("kdj", Ctx::kdj);
    engine.register_fn("boll", Ctx::boll);

    // 账户 / 交易
    engine.register_fn("position", Ctx::position);
    engine.register_fn("cash", Ctx::cash);
    engine.register_fn("total_value", Ctx::total_value);
    engine.register_fn("max_shares", Ctx::max_shares);
    engine.register_fn("buy", Ctx::buy);
    engine.register_fn("sell", Ctx::sell);

    engine
}

pub struct Strategy {
    engine: Engine,
    ast: AST,
}

impl Strategy {
    pub fn load(path: &str) -> Result<Strategy> {
        let src =
            std::fs::read_to_string(path).with_context(|| format!("无法读取策略脚本 {}", path))?;
        let engine = build_engine();
        let ast = engine
            .compile(&src)
            .map_err(|e| anyhow!("策略脚本编译失败：{}", e))?;
        Ok(Strategy { engine, ast })
    }

    /// 读取脚本顶层的 name 常量，作为策略名。
    pub fn name(&self) -> String {
        let mut scope = Scope::new();
        self.engine
            .eval_ast_with_scope::<()>(&mut scope, &self.ast)
            .ok();
        scope
            .get_value::<String>("name")
            .unwrap_or_else(|| "Strategy".to_string())
    }

    /// 对单根 bar 调用脚本的 on_bar 函数。
    pub fn call_on_bar(&self, scope: &mut Scope, ctx: Ctx) -> Result<()> {
        self.engine
            .call_fn::<()>(scope, &self.ast, "on_bar", (ctx,))
            .map_err(|e| anyhow!("on_bar 执行出错：{}", e))?;
        Ok(())
    }
}
