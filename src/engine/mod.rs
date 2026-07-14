pub mod backtest;
pub mod context;
pub mod metrics;
pub mod portfolio;
pub mod position;

pub use backtest::{run, BacktestResult};
pub use context::Ctx;
pub use metrics::compute_benchmark;
pub use position::compute as position_stats;
