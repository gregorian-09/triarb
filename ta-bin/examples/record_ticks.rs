//! Record live Binance top-of-book ticks to a JSONL file.
//!
//! Usage:
//!   cargo run --example record_ticks -- [--duration 60] [--interval 100] [--output ticks.jsonl]
//!
//! No API keys required — uses Binance public WebSocket.

use std::path::PathBuf;
use std::time::Instant;
use std::io::Write;

use clap::Parser;
use of_core::SymbolId;
use ta_feed::FeedEngine;
use ta_sim::RawTick;

#[derive(Parser)]
struct Args {
    /// Recording duration in seconds.
    #[arg(long, default_value = "60")]
    duration: u64,

    /// Poll interval in milliseconds.
    #[arg(long, default_value = "100")]
    interval_ms: u64,

    /// Output JSONL path.
    #[arg(long, default_value = "ticks.jsonl")]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,want=off,supervisor=off".to_string()),
        )
        .init();

    let args = Args::parse();
    let endpoint = "wss://stream.binance.com:9443/ws";

    let symbols = [
        ("BTCUSDT", "BINANCE"),
        ("ETHUSDT", "BINANCE"),
        ("ETHBTC", "BINANCE"),
    ];

    let mut feed = FeedEngine::new(Some(endpoint.to_string()));
    tracing::info!("connecting to {endpoint} ...");
    feed.connect().await;

    for (sym, venue) in &symbols {
        let id = SymbolId {
            venue: venue.to_string(),
            symbol: sym.to_string(),
        };
        feed.subscribe(id).await;
        tracing::info!("subscribed to {sym}");
    }

    // Wait a moment for the initial snapshots
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let start = Instant::now();
    let duration = std::time::Duration::from_secs(args.duration);
    let mut cycle: u64 = 0;
    let mut written: u64 = 0;

    let file = std::fs::File::create(&args.output)?;
    let mut writer = std::io::BufWriter::new(file);

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(args.interval_ms)).await;
        feed.poll().await;
        cycle += 1;

        let elapsed = start.elapsed();
        if elapsed > duration {
            break;
        }

        let reader = feed.book_reader();
        let books = reader.read().await;

        // Write a tick for each symbol every poll cycle
        for (sym, book) in books.iter() {
            let bid = book.bids.first().map(|l| l.price).unwrap_or(0);
            let ask = book.asks.first().map(|l| l.price).unwrap_or(0);

            if bid == 0 || ask == 0 {
                continue;
            }

            let tick = RawTick {
                ts_ns: now_ns(),
                venue: sym.venue.clone(),
                symbol: sym.symbol.clone(),
                bid,
                ask,
                last_price: bid,
                last_size: book.bids.first().map(|l| l.size).unwrap_or(0),
            };

            let line = serde_json::to_string(&tick)?;
            writeln!(writer, "{}", line)?;
            written += 1;
        }

        if cycle % 10 == 0 {
            tracing::info!(
                "t={:3}s  cycles={}  ticks={}",
                elapsed.as_secs(),
                cycle,
                written,
            );
        }
    }

    writer.flush()?;
    drop(writer);

    let health = feed.health();
    tracing::info!(
        "done — {} poll cycles, {} ticks written to {} (connected={}, errors={})",
        cycle,
        written,
        args.output.display(),
        health.connected,
        health.consecutive_errors,
    );

    Ok(())
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
