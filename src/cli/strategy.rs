use std::path::Path;

use anyhow::{anyhow, Result};
use clap::Subcommand;

/// `strategy new` 写出的模板:内联注释即完整 ctx API 文档,离线且跟版本走。
/// 与 rhai_engine::build_engine 注册的方法一一对应,改引擎时记得同步这里。
const TEMPLATE: &str = include_str!("strategy_template.rhai");

#[derive(Subcommand)]
pub enum StrategyCmd {
    /// 生成一个带完整 ctx API 注释的策略模板（脚手架）
    New {
        /// 目标文件路径，如 my_strategy.rhai
        file: String,
    },
}

pub fn run(cmd: StrategyCmd) -> Result<()> {
    match cmd {
        StrategyCmd::New { file } => new(&file),
    }
}

fn new(file: &str) -> Result<()> {
    if Path::new(file).exists() {
        return Err(anyhow!("{} 已存在，换个文件名以免覆盖", file));
    }
    std::fs::write(file, TEMPLATE).map_err(|e| anyhow!("无法写入 {}：{}", file, e))?;
    println!("已生成策略模板 {}", file);
    println!("模板含完整 ctx API 注释，改掉 on_bar 即可。回测：");
    println!("  stockrs backtest {file} --stock 600519 --start 2023-01-01");
    Ok(())
}
