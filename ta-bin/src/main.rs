mod backtest;
mod cli;
mod config;
mod metrics;

use clap::Parser;
use config::AppConfig;
use metrics::serve_metrics;
use of_core::SymbolId;
use std::time::Instant;
use ta_config::{Config, ConfigError};
use ta_detect::{DetectionConfig, DetectionEngine};
use ta_exec::ExecEngine;
use ta_feed::{FeedConfig, FeedEngine};
use tokio::signal;

fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();

    match args.command.as_ref().unwrap_or(&cli::Command::Live) {
        cli::Command::Live => run_live(args),
        cli::Command::Backtest(bt_args) => backtest::run_backtest(bt_args),
    }
}

fn run_live(args: cli::Cli) -> anyhow::Result<()> {
    // Load app config (symbols, endpoints, logging, etc.)
    let app_cfg = match &args.config {
        Some(path) => AppConfig::from_file(path.as_path())?,
        None => AppConfig::default(),
    };

    // Initialize logging
    init_logging(&app_cfg);

    // Load exchange credentials
    let creds = load_config(&args).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "no exchange credentials — running in read-only mode");
        None
    });
    if let Some(ref creds) = creds {
        creds.export_env();
    }

    tracing::info!("triangular arbitrage bot starting");
    tracing::info!(symbols = ?app_cfg.symbols, endpoint = ?app_cfg.endpoint, "config");

    // Start metrics + health HTTP server
    let rt = tokio::runtime::Runtime::new()?;
    let metrics_port = app_cfg.metrics_port;
    rt.block_on(async {
        tokio::spawn(async move {
            serve_metrics(metrics_port).await;
        });
        tokio::select! {
            _ = run_loop(app_cfg) => {}
            _ = shutdown_signal() => {
                tracing::info!("shutdown signal received, draining...");
                drain().await;
            }
        }
    });

    tracing::info!("bot stopped cleanly");
    Ok(())
}

fn init_logging(cfg: &AppConfig) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.logging.level));

    if let Some(ref dir) = cfg.logging.directory {
        let file_appender = tracing_appender::rolling::daily(dir, "ta-bot.log");
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        // Keep guard alive for the process lifetime
        Box::leak(Box::new(_guard));

        match cfg.logging.format.as_str() {
            "json" => {
                let subscriber = tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(filter)
                    .with_writer(non_blocking)
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
            }
            _ => {
                let subscriber = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(non_blocking)
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
            }
        }
    } else {
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    }
}

async fn run_loop(app_cfg: AppConfig) {
    // Initialize feed
    let mut feed = FeedEngine::with_config(FeedConfig {
        endpoint: app_cfg.endpoint.clone(),
        message_timeout: app_cfg.message_timeout(),
        price_scale: 1_000_000.0,
        reconnect_base_ms: app_cfg.feed.reconnect_base_ms,
        reconnect_max_ms: app_cfg.feed.reconnect_max_ms,
    });

    // Subscribe to configured symbols
    for sym_str in &app_cfg.symbols {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: sym_str.to_string(),
        };
        feed.subscribe(symbol).await;
    }

    // Connect to the exchange
    feed.connect().await;

    // Initialize detection engine
    let detect = DetectionEngine::new(DetectionConfig {
        min_profit_bps: app_cfg.detect.min_profit_bps,
        max_legs: app_cfg.detect.max_legs,
        fee_taker_bps: app_cfg.detect.fee_taker_bps,
        max_data_age: app_cfg.max_data_age(),
    });

    // Initialize execution engine (no-op if no credentials)
    let mut exec = ExecEngine::new(Default::default());

    let book_reader = feed.book_reader();
    let graph_reader = feed.graph_reader();

    let mut health_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut timeout_interval = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut poll_interval = tokio::time::interval(app_cfg.poll_interval());
    let mut prev_books: u64 = 0;
    let mut prev_trades: u64 = 0;

    loop {
        tokio::select! {
            _ = health_interval.tick() => {
                let health = feed.health();
                metrics::metrics().feed_connected.set(if health.connected { 1.0 } else { 0.0 });
                if health.consecutive_errors > 0 {
                    metrics::metrics().feed_reconnects.inc();
                }
                if health.degraded {
                    tracing::warn!(
                        connected = health.connected,
                        stale = health.last_message_at.map(|t| t.elapsed().as_secs()).unwrap_or(99),
                        errors = health.consecutive_errors,
                        books = feed.counters().books,
                        trades = feed.counters().trades,
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
            _ = poll_interval.tick() => {
                feed.poll().await;
                metrics::metrics().polls_total.inc();
                let cnt = feed.counters();
                let books_delta = cnt.books - prev_books;
                let trades_delta = cnt.trades - prev_trades;
                if books_delta > 0 {
                    metrics::metrics().books_received.inc_by(books_delta as f64);
                }
                if trades_delta > 0 {
                    metrics::metrics().trades_received.inc_by(trades_delta as f64);
                }
                prev_books = cnt.books;
                prev_trades = cnt.trades;

                let graph_snap = graph_reader.read().await;
                let detect_start = Instant::now();
                let opportunities = detect.detect(&graph_snap);
                let detect_elapsed = detect_start.elapsed();
                metrics::metrics().detection_duration
                    .with_label_values(&[])
                    .observe(detect_elapsed.as_secs_f64());
                drop(graph_snap);

                if opportunities.is_empty() {
                    continue;
                }

                metrics::metrics().opportunities_found.inc_by(opportunities.len() as f64);

                let books_snap = book_reader.read().await;

                for opp in opportunities {
                    let enriched = enrich_opportunity(&opp, &books_snap, 1_000_000);

                    if enriched.routes.iter().any(|l| l.price == 0 || l.size == 0) {
                        tracing::debug!(?enriched.triangle, "skipping — missing price/size");
                        continue;
                    }

                    match exec.execute_opportunity(&enriched, &books_snap) {
                        Ok(result) => {
                            metrics::metrics().opportunities_executed.inc();
                            tracing::info!(
                                profit_bps = enriched.expected_profit_bps,
                                ?result,
                                "opportunity executed"
                            );
                        }
                        Err(e) => {
                            metrics::metrics().executions_failed.inc();
                            tracing::error!(error = %e, "execution failed");
                        }
                    }
                }
            }
        }
    }
}

fn enrich_opportunity(
    opp: &ta_core::ArbitrageOpportunity,
    books: &rustc_hash::FxHashMap<of_core::SymbolId, of_core::BookSnapshot>,
    max_size: i64,
) -> ta_core::ArbitrageOpportunity {
    use ta_core::OrderSide;
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
    ta_core::ArbitrageOpportunity {
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

fn load_config(args: &cli::Cli) -> Result<Option<Config>, ConfigError> {
    #[cfg(feature = "aws")]
    if let Some(secret_name) = &args.aws_secret {
        tracing::info!("loading config from AWS Secrets Manager");
        let rt = tokio::runtime::Runtime::new().unwrap();
        return rt.block_on(async { Config::from_aws(secret_name).await.map(Some) });
    }

    #[cfg(not(feature = "aws"))]
    if args.aws_secret.is_some() {
        tracing::warn!("aws-secret flag requires 'aws' feature: cargo build --features aws");
    }

    // Make credentials optional — allow running without API keys for read-only mode
    if let Ok(cfg) = Config::from_env(args.env_file.as_deref()) {
        tracing::info!("exchange credentials loaded from environment");
        return Ok(Some(cfg));
    }

    tracing::info!("no credentials found — running in read-only mode");
    Ok(None)
}
