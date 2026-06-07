use clap::Parser;

#[derive(Parser)]
#[command(name = "ta", about = "Triangular arbitrage bot")]
pub struct Cli {
    #[arg(long, default_value = "live")]
    pub mode: String,

    /// Path to a .env file for local development.
    /// If not set, reads from process environment.
    #[arg(long)]
    pub env_file: Option<String>,

    /// AWS Secrets Manager secret name (production).
    /// Requires ta-config built with the "aws" feature.
    #[arg(long)]
    pub aws_secret: Option<String>,
}
