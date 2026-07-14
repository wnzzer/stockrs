pub mod backtest;
pub mod context;
pub mod metrics;
pub mod position;

pub use backtest::{run, BacktestResult};
pub use context::Ctx;
pub use position::compute as position_stats;
