mod cli;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let _args = cli::Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("triangular arbitrage bot starting");
    Ok(())
}
