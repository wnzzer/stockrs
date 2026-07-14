mod cli;
mod data;
mod engine;
mod indicator;
mod strategy;
mod utils;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() {
    if let Err(e) = cli::run(Cli::parse()).await {
        eprintln!("错误：{:#}", e);
        std::process::exit(1);
    }
}
