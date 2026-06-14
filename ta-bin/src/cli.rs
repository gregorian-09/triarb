use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ta", about = "Triangular arbitrage bot")]
pub struct Cli {
    /// Path to the production TOML configuration file.
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
