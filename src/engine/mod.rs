pub mod backtest;
pub mod context;
pub mod metrics;

pub use backtest::{run, BacktestResult};
pub use context::Ctx;
