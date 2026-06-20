//! Live Binance public WebSocket feed demo.
//!
//! Connects to Binance public WebSocket (no API keys needed), subscribes to
//! BTCUSDT, ETHUSDT, ETHBTC, and prints top-of-book updates every second
//! for 30 seconds. Demonstrates the ta-feed engine working with live data.

use of_core::SymbolId;
use std::time::Instant;
use ta_feed::FeedEngine;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,want=off,supervisor=off".to_string()),
        )
        .init();

    let endpoint = "wss://stream.binance.com:9443/ws";

    let symbols = [
        ("BTCUSDT", "BTC", "USDT"),
        ("ETHUSDT", "ETH", "USDT"),
        ("ETHBTC", "ETH", "BTC"),
    ];

    let mut feed = FeedEngine::new(Some(endpoint.to_string()));

    tracing::info!("connecting to {endpoint} ...");
    feed.connect().await;

    for (sym, base, quote) in &symbols {
        let id = SymbolId {
            venue: "BINANCE".into(),
            symbol: sym.to_string(),
        };
        feed.subscribe(id).await;
        tracing::info!("subscribed to {sym} ({base}/{quote})");
    }

    let start = Instant::now();
    let duration = std::time::Duration::from_secs(30);
    let mut cycle: u64 = 0;

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        feed.poll().await;
        cycle += 1;

        let elapsed = start.elapsed();
        if elapsed > duration {
            break;
        }

        if !cycle.is_multiple_of(10) {
            continue;
        }

        let health = feed.health();
        let books_rw = feed.book_reader();
        let graph_rw = feed.graph_reader();
        let books = books_rw.read().await;
        let graph = graph_rw.read().await;

        tracing::info!(
            "--- t={}s  currencies={}  connected={}  errors={} ---",
            elapsed.as_secs(),
            graph.currencies().len(),
            health.connected,
            health.consecutive_errors,
        );

        for (sym, book) in books.iter() {
            let bid = book.bids.first().map(|l| l.price).unwrap_or(0);
            let ask = book.asks.first().map(|l| l.price).unwrap_or(0);
            let bid_sz = book.bids.first().map(|l| l.size).unwrap_or(0);
            let ask_sz = book.asks.first().map(|l| l.size).unwrap_or(0);
            tracing::info!(
                "  {:<8}  bid={:>15} ({:>5})  ask={:>15} ({:>5})  levels={}/{}",
                sym.symbol,
                bid,
                bid_sz,
                ask,
                ask_sz,
                book.bids.len(),
                book.asks.len(),
            );
        }
    }

    tracing::info!(
        "done — {} poll cycles in 30s (connected={}, degraded={})",
        cycle,
        feed.health().connected,
        feed.health().degraded,
    );
}
