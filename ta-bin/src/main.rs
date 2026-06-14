mod cli;

use clap::Parser;
use of_core::{BookSnapshot, SymbolId};
use rustc_hash::FxHashMap;
use std::time::Duration;
use ta_config::{Config, ConfigError};
use ta_core::{ArbitrageOpportunity, OrderSide};
use ta_detect::{DetectionConfig, DetectionEngine};
use ta_exec::{check_ntp, ExecEngine, DEFAULT_NTP_SERVER, MAX_CLOCK_SKEW_MS};
use ta_feed::{FeedConfig, FeedEngine};
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

    // Check NTP clock sync before touching any exchange API
    match tokio::task::spawn_blocking(|| check_ntp(DEFAULT_NTP_SERVER)).await {
        Ok(Some(skew)) => {
            if skew.is_safe() {
                tracing::info!(
                    offset_ms = skew.offset_ms,
                    delay_ms = skew.delay_ms,
                    "NTP clock check passed"
                );
            } else {
                tracing::error!(
                    offset_ms = skew.offset_ms,
                    max_skew_ms = MAX_CLOCK_SKEW_MS,
                    "clock skew exceeds safe threshold — refusing to start"
                );
                return;
            }
        }
        Ok(None) => {
            tracing::warn!("NTP check failed (network or DNS) — proceeding without verification");
        }
        Err(e) => {
            tracing::warn!("NTP check task failed: {e} — proceeding");
        }
    }

    // Initialize feed with health check config
    let mut feed = FeedEngine::with_config(FeedConfig {
        message_timeout: Duration::from_secs(10),
        ..Default::default()
    });

    // Initialize detection engine
    let detect = DetectionEngine::new(DetectionConfig {
        min_profit_bps: 10.0,
        max_legs: 3,
        fee_taker_bps: 10.0,
        max_data_age: Duration::from_millis(200),
    });

    // Initialize execution engine
    let mut exec = ExecEngine::new(Default::default());

    // Clone Arc readers for book and graph (shared with feed)
    let book_reader = feed.book_reader();
    let graph_reader = feed.graph_reader();

    // Connect to the exchange
    feed.connect().await;

    let mut health_interval = tokio::time::interval(Duration::from_secs(5));
    let mut timeout_interval = tokio::time::interval(Duration::from_secs(2));
    let mut detect_interval = tokio::time::interval(Duration::from_millis(50));

    loop {
        tokio::select! {
            _ = health_interval.tick() => {
                let health = feed.health();
                if health.degraded {
                    tracing::warn!(
                        connected = health.connected,
                        stale = health.last_message_at.map(|t| t.elapsed().as_secs()).unwrap_or(99),
                        errors = health.consecutive_errors,
                        "feed degraded"
                    );
                } else if health.connected {
                    tracing::debug!("feed healthy");
                }
            }
            _ = timeout_interval.tick() => {
                let timeouts = exec.check_order_timeouts();
                if !timeouts.is_empty() {
                    tracing::warn!("{} order(s) timed out", timeouts.len());
                }
            }
            _ = detect_interval.tick() => {
                // 1. Poll feed for new market data
                feed.poll().await;

                // 2. Snapshot the graph for detection
                let graph_snap = graph_reader.read().await;
                let opportunities = detect.detect(&graph_snap);
                drop(graph_snap);

                if opportunities.is_empty() {
                    continue;
                }

                // 3. Snapshot books for price enrichment
                let books_snap = book_reader.read().await;

                for opp in opportunities {
                    let enriched = enrich_opportunity(&opp, &books_snap, 1_000_000);

                    // Skip if any leg has no price or size
                    if enriched.routes.iter().any(|l| l.price == 0 || l.size == 0) {
                        tracing::debug!(?enriched.triangle, "skipping opportunity — missing price/size");
                        continue;
                    }

                    // 4. Execute
                    match exec.execute_opportunity(&enriched, &books_snap) {
                        Ok(result) => {
                            tracing::info!(
                                profit_bps = enriched.expected_profit_bps,
                                ?result,
                                "opportunity executed"
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "execution failed");
                        }
                    }
                }
            }
        }
    }
}

/// Fill in `price` and `size` for each leg from the current top-of-book.
fn enrich_opportunity(
    opp: &ArbitrageOpportunity,
    books: &FxHashMap<SymbolId, BookSnapshot>,
    max_size: i64,
) -> ArbitrageOpportunity {
    let mut routes = opp.routes.clone();
    for leg in &mut routes {
        if let Some(book) = books.get(&leg.symbol) {
            match leg.side {
                OrderSide::Buy => {
                    if let Some(ask) = book.asks.first() {
                        leg.price = ask.price;
                        leg.size = ask.size.min(max_size);
                    }
                }
                OrderSide::Sell => {
                    if let Some(bid) = book.bids.first() {
                        leg.price = bid.price;
                        leg.size = bid.size.min(max_size);
                    }
                }
            }
        }
    }
    ArbitrageOpportunity {
        triangle: opp.triangle.clone(),
        routes,
        expected_profit_bps: opp.expected_profit_bps,
        ts_ns: opp.ts_ns,
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
