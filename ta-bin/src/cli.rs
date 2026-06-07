use clap::Parser;

#[derive(Parser)]
#[command(name = "ta", about = "Triangular arbitrage bot")]
pub struct Cli {
    #[arg(long, default_value = "live")]
    pub mode: String,

    #[arg(long)]
    pub config: Option<String>,
}
