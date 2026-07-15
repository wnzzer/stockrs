pub mod backtest;
pub mod data;
pub mod indicator;
pub mod portfolio;
pub mod quote;
pub mod selfupdate;
pub mod strategy;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "stockrs",
    version,
    about = "轻量 A 股量化 CLI 工具",
    after_help = "上手:\n  stockrs data add 600519          下载日K(港股用 hk00700)\n  stockrs strategy new my.rhai     新建策略模板(含完整 ctx API 注释)\n  stockrs backtest my.rhai --stock 600519\n\n各子命令用 `stockrs <命令> --help` 看细节。完整文档见 README:\nhttps://github.com/wnzzer/stockrs"
)]
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
    #[command(after_help = "PE/PB 依次取自 东财实时 → 腾讯实时(A股/港股) → 本地基本面;\n带 * 者来自本地基本面(截至最近收盘),实时源未提供。")]
    Quote {
        /// 一个或多个股票代码
        codes: Vec<String>,
    },

    /// 技术指标展示
    Indicator {
        /// 股票/基金代码（港股用 hk00700 或 00700）
        code: String,
        /// 指标周期（默认 20）
        #[arg(long, default_value_t = 20)]
        period: usize,
    },

    /// 回测策略脚本（单标的 / 多股票组合 / 参数扫描）
    #[command(
        after_help = "示例:\n  单标的    stockrs backtest strategies/sma_cross.rhai --stock 600519 --start 2023-01-01\n  港股      stockrs backtest strategies/sma_cross.rhai --stock hk00700\n  组合      stockrs backtest strategies/momentum_rotation.rhai --universe\n  基准对比  stockrs backtest strategies/sma_cross.rhai --stock 600519 --benchmark hs300\n  参数扫描  stockrs backtest strategies/sma_cross_param.rhai --stock 600519 --param fast=5,10 --optimize sharpe\n\n新建策略: stockrs strategy new my.rhai   (生成带完整 ctx API 注释的模板)"
    )]
    Backtest {
        /// Rhai 策略脚本路径
        script: String,
        /// 单标的回测代码
        #[arg(long)]
        stock: Option<String>,
        /// 组合回测股票列表（逗号分隔或多次指定），与 --stock 互斥
        #[arg(long, value_delimiter = ',')]
        stocks: Vec<String>,
        /// 组合回测使用全部已跟踪股票
        #[arg(long)]
        universe: bool,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long, default_value_t = 100_000.0)]
        capital: f64,
        /// 基准指数：hs300/zz500/sh/sz/cyb 等别名或指数代码
        #[arg(long)]
        benchmark: Option<String>,
        /// 参数扫描：key=v1,v2,...（可重复指定多个参数做网格）
        #[arg(long)]
        param: Vec<String>,
        /// 扫描排序键：return（默认）/annual/sharpe/drawdown
        #[arg(long)]
        optimize: Option<String>,
    },

    /// 持仓管理
    #[command(subcommand)]
    Portfolio(portfolio::PortfolioCmd),

    /// 策略脚手架（生成带 ctx API 注释的模板）
    #[command(subcommand)]
    Strategy(strategy::StrategyCmd),

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
            stocks,
            universe,
            start,
            end,
            capital,
            benchmark,
            param,
            optimize,
        } => {
            backtest::run(
                script, stock, stocks, universe, start, end, capital, benchmark, param, optimize,
            )
            .await
        }
        Commands::Portfolio(cmd) => portfolio::run(cmd).await,
        Commands::Strategy(cmd) => strategy::run(cmd),
        Commands::SelfUpdate { check } => selfupdate::run(check).await,
    }
}
