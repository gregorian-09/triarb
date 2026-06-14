use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ta", about = "Triangular arbitrage bot")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to the production TOML configuration file (live mode).
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Path to a .env file for local development.
    /// If not set, reads from process environment.
    #[arg(long)]
    pub env_file: Option<String>,

    /// AWS Secrets Manager secret name (production).
    /// Requires ta-config built with the "aws" feature.
    #[arg(long)]
    pub aws_secret: Option<String>,

    /// Metrics HTTP server port.
    #[arg(long, default_value = "9100")]
    pub metrics_port: u16,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the live trading bot (default).
    Live,
    /// Run a backtest over historical tick data.
    #[command(name = "backtest")]
    Backtest(BacktestArgs),
}

#[derive(clap::Args)]
pub struct BacktestArgs {
    /// Path to the backtest TOML configuration file.
    #[arg(long)]
    pub config: PathBuf,
    /// Path to historical tick JSONL data (overrides config).
    #[arg(long)]
    pub data: Option<PathBuf>,
    /// Output path for trade JSONL (overrides config).
    #[arg(long)]
    pub output: Option<PathBuf>,
}
