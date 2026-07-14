pub mod backtest;
pub mod data;
pub mod indicator;
pub mod portfolio;
pub mod quote;
pub mod selfupdate;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "stockrs", version, about = "轻量 A 股量化 CLI 工具")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 数据管理（跟踪列表与日K维护）
    #[command(subcommand)]
    Data(data::DataCmd),

    /// 实时行情查询
    Quote {
        /// 一个或多个股票代码
        codes: Vec<String>,
    },

    /// 技术指标展示
    Indicator {
        code: String,
        #[arg(long, default_value_t = 20)]
        period: usize,
    },

    /// 回测策略脚本
    Backtest {
        /// Rhai 策略脚本路径
        script: String,
        #[arg(long)]
        stock: String,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long, default_value_t = 100_000.0)]
        capital: f64,
    },

    /// 持仓管理
    #[command(subcommand)]
    Portfolio(portfolio::PortfolioCmd),

    /// 更新 stockrs 自身到最新版本
    #[command(name = "self-update")]
    SelfUpdate {
        /// 只检查是否有新版本，不实际更新
        #[arg(long)]
        check: bool,
    },
}

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::Data(cmd) => data::run(cmd).await,
        Commands::Quote { codes } => quote::run(codes).await,
        Commands::Indicator { code, period } => indicator::run(code, period),
        Commands::Backtest {
            script,
            stock,
            start,
            end,
            capital,
        } => backtest::run(script, stock, start, end, capital),
        Commands::Portfolio(cmd) => portfolio::run(cmd).await,
        Commands::SelfUpdate { check } => selfupdate::run(check).await,
    }
}
