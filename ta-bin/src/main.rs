mod cli;

use clap::Parser;
use ta_config::{Config, ConfigError};
use tokio::signal;

fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = load_config(&args)?;
    config.export_env();
    tracing::info!("config loaded");

    tracing::info!("triangular arbitrage bot starting");

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        tokio::select! {
            _ = run_loop() => {}
            _ = shutdown_signal() => {
                tracing::info!("shutdown signal received, draining...");
                drain().await;
            }
        }
    });

    tracing::info!("bot stopped cleanly");
    Ok(())
}

async fn run_loop() {
    tracing::info!("main loop started");
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

async fn shutdown_signal() {
    signal::ctrl_c().await.expect("failed to listen for ctrl-c");
    tracing::info!("ctrl-c received");
}

async fn drain() {
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    tracing::info!("pending state drained");
}

fn load_config(args: &cli::Cli) -> Result<Config, ConfigError> {
    #[cfg(feature = "aws")]
    if let Some(secret_name) = &args.aws_secret {
        tracing::info!("loading config from AWS Secrets Manager");
        let rt = tokio::runtime::Runtime::new().unwrap();
        return rt.block_on(async { Config::from_aws(secret_name).await });
    }

    #[cfg(not(feature = "aws"))]
    if args.aws_secret.is_some() {
        tracing::warn!("aws-secret flag requires 'aws' feature: cargo build --features aws");
    }

    tracing::info!("loading config from environment");
    Config::from_env(args.env_file.as_deref())
}
