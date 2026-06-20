use of_adapters::{AdapterConfig, MarketDataAdapter, ProviderKind, RawEvent, SubscribeReq};
use of_core::{BookLevel, BookSnapshot, BookUpdate, Side, SymbolId, TradePrint};
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ta_core::ExchangeRateGraph;
use tokio::sync::RwLock;

const TOP_DEPTH: u16 = 10;

/// Configuration for the feed engine.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// Optional WebSocket endpoint override.
    pub endpoint: Option<String>,
    /// Maximum time since the last message before the feed is considered stale.
    pub message_timeout: Duration,
    /// Divide raw integer prices by this factor to obtain natural-unit prices
    /// before inserting into the exchange-rate graph.  Binance adapter prices
    /// typically carry a 1 000 000× scale factor for USDT pairs.
    pub price_scale: f64,
    /// Base reconnection delay in milliseconds (exponential backoff).
    pub reconnect_base_ms: u64,
    /// Maximum reconnection delay in milliseconds.
    pub reconnect_max_ms: u64,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            message_timeout: Duration::from_secs(10),
            price_scale: 1_000_000.0,
            reconnect_base_ms: DEFAULT_RECONNECT_BASE_MS,
            reconnect_max_ms: DEFAULT_RECONNECT_MAX_MS,
        }
    }
}

/// Connection health snapshot for a feed.
#[derive(Debug, Clone)]
pub struct FeedHealth {
    pub connected: bool,
    pub last_message_at: Option<Instant>,
    pub consecutive_errors: u32,
    pub degraded: bool,
}

impl FeedHealth {
    /// Returns true if no message has been received within `max_age`.
    pub fn is_stale(&self, max_age: Duration) -> bool {
        match self.last_message_at {
            Some(t) => t.elapsed() > max_age,
            None => true,
        }
    }
}

pub(crate) const DEFAULT_RECONNECT_BASE_MS: u64 = 250;
pub(crate) const DEFAULT_RECONNECT_MAX_MS: u64 = 30_000;

#[derive(Default)]
pub struct FeedCounters {
    pub books: u64,
    pub trades: u64,
}

pub struct FeedEngine {
    adapter: Box<dyn MarketDataAdapter + Send>,
    books: Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>>,
    graph: Arc<RwLock<ExchangeRateGraph>>,
    subscribed: Vec<SymbolId>,
    connected: bool,
    last_message_at: Option<Instant>,
    consecutive_errors: u32,
    config: FeedConfig,
    reconnect_attempt: u32,
    next_reconnect_at: Option<Instant>,
    counters: FeedCounters,
}

impl FeedEngine {
    pub fn new(endpoint: Option<String>) -> Self {
        Self::with_config(FeedConfig {
            endpoint,
            ..Default::default()
        })
    }

    pub fn with_config(config: FeedConfig) -> Self {
        let cfg = AdapterConfig {
            provider: ProviderKind::Binance,
            endpoint: config.endpoint.clone(),
            ..Default::default()
        };
        let adapter = of_adapters::create_adapter(&cfg).expect("create Binance adapter");
        Self {
            adapter,
            books: Arc::new(RwLock::new(FxHashMap::default())),
            graph: Arc::new(RwLock::new(ExchangeRateGraph::new())),
            subscribed: Vec::new(),
            connected: false,
            last_message_at: None,
            consecutive_errors: 0,
            config,
            reconnect_attempt: 0,
            next_reconnect_at: None,
            counters: FeedCounters::default(),
        }
    }

    pub fn book_reader(&self) -> Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>> {
        self.books.clone()
    }

    pub fn graph_reader(&self) -> Arc<RwLock<ExchangeRateGraph>> {
        self.graph.clone()
    }

    pub fn counters(&self) -> &FeedCounters {
        &self.counters
    }

    /// Returns a snapshot of current feed health.
    pub fn health(&self) -> FeedHealth {
        FeedHealth {
            connected: self.connected,
            last_message_at: self.last_message_at,
            consecutive_errors: self.consecutive_errors,
            degraded: !self.connected || self.consecutive_errors >= 5 || self.is_stale(),
        }
    }

    fn is_stale(&self) -> bool {
        match self.last_message_at {
            Some(t) => t.elapsed() > self.config.message_timeout,
            None => true,
        }
    }

    pub async fn connect(&mut self) {
        match self.adapter.connect() {
            Ok(_) => {
                self.connected = true;
                self.consecutive_errors = 0;
                self.reconnect_attempt = 0;
                self.next_reconnect_at = None;
                tracing::info!("feed connected");
            }
            Err(e) => {
                self.connected = false;
                self.consecutive_errors += 1;
                tracing::error!("feed connect failed: {e}");
                self.schedule_reconnect();
            }
        }
    }

    pub async fn try_reconnect(&mut self) {
        if self.connected {
            return;
        }
        let due = self
            .next_reconnect_at
            .map(|t| Instant::now() >= t)
            .unwrap_or(true);
        if !due {
            return;
        }
        tracing::info!(
            attempt = self.reconnect_attempt,
            "attempting feed reconnect"
        );
        if self.adapter.connect().is_ok() {
            self.connected = true;
            self.consecutive_errors = 0;
            self.reconnect_attempt = 0;
            self.next_reconnect_at = None;
            tracing::info!("feed reconnected");
            for sym in &self.subscribed {
                let _ = self.adapter.subscribe(SubscribeReq {
                    symbol: sym.clone(),
                    depth_levels: TOP_DEPTH,
                });
            }
        } else {
            self.consecutive_errors += 1;
            self.schedule_reconnect();
        }
    }

    fn schedule_reconnect(&mut self) {
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        let base = self.config.reconnect_base_ms;
        let max = self.config.reconnect_max_ms;
        let delay = (base.saturating_mul(1u64 << self.reconnect_attempt.min(7))).min(max);
        self.next_reconnect_at = Some(Instant::now() + Duration::from_millis(delay));
        tracing::warn!(
            attempt = self.reconnect_attempt,
            delay_ms = delay,
            "feed reconnect scheduled"
        );
    }

    pub async fn subscribe(&mut self, symbol: SymbolId) {
        match self.adapter.subscribe(SubscribeReq {
            symbol: symbol.clone(),
            depth_levels: TOP_DEPTH,
        }) {
            Ok(_) => {
                self.subscribed.push(symbol);
            }
            Err(e) => {
                self.consecutive_errors += 1;
                tracing::error!("feed subscribe failed: {e}");
            }
        }
    }

    pub async fn poll(&mut self) {
        // Attempt reconnection if disconnected
        if !self.connected {
            self.try_reconnect().await;
            return;
        }

        let mut events = Vec::new();
        match self.adapter.poll(&mut events) {
            Ok(_) => {
                self.consecutive_errors = 0;
            }
            Err(e) => {
                self.consecutive_errors += 1;
                tracing::warn!(
                    "poll error ({}/5 consecutive): {e}",
                    self.consecutive_errors
                );
                if self.consecutive_errors >= 5 {
                    tracing::error!("feed degraded: too many consecutive poll errors");
                    self.connected = false;
                    self.schedule_reconnect();
                }
                return;
            }
        }

        if events.is_empty() {
            return;
        }

        self.last_message_at = Some(Instant::now());

        let mut books = self.books.write().await;
        let mut graph = self.graph.write().await;

        for event in events {
            match event {
                RawEvent::Book(_update) => {
                    self.counters.books += 1;
                    Self::apply_book_update(&mut books, _update, &mut graph, self.config.price_scale);
                }
                RawEvent::Trade(trade) => {
                    self.counters.trades += 1;
                    Self::apply_trade(&mut books, trade);
                }
            }
        }
    }

    pub fn bench_apply_book_update(
        books: &mut FxHashMap<SymbolId, BookSnapshot>,
        graph: &mut ExchangeRateGraph,
        update: BookUpdate,
        price_scale: f64,
    ) {
        FeedEngine::apply_book_update(books, update, graph, price_scale);
    }

    fn apply_book_update(
        books: &mut FxHashMap<SymbolId, BookSnapshot>,
        update: BookUpdate,
        graph: &mut ExchangeRateGraph,
        price_scale: f64,
    ) {
        let snap = books.entry(update.symbol.clone()).or_insert_with(|| {
            let currency = parse_currency(&update.symbol);
            let base = currency.0;
            let quote = currency.1;
            graph.add_currency(base);
            graph.add_currency(quote);
            BookSnapshot {
                symbol: update.symbol.clone(),
                bids: Vec::new(),
                asks: Vec::new(),
                last_sequence: 0,
                ts_exchange_ns: 0,
                ts_recv_ns: 0,
            }
        });

        let levels = match update.side {
            Side::Bid => &mut snap.bids,
            Side::Ask => &mut snap.asks,
        };

        match update.action {
            of_core::BookAction::Upsert => {
                if let Some(existing) = levels
                    .iter_mut()
                    .find(|l: &&mut BookLevel| l.level == update.level)
                {
                    existing.price = update.price;
                    existing.size = update.size;
                } else {
                    levels.push(BookLevel {
                        price: update.price,
                        size: update.size,
                        level: update.level,
                    });
                }
                levels.sort_by_key(|l| l.level);
            }
            of_core::BookAction::Delete => {
                levels.retain(|l| l.level != update.level);
            }
        }

        snap.last_sequence = update.sequence;
        snap.ts_exchange_ns = update.ts_exchange_ns;
        snap.ts_recv_ns = update.ts_recv_ns;

        if let (Some(bid), Some(ask)) = (snap.bids.first(), snap.asks.first()) {
            let currency = parse_currency(&snap.symbol);
            graph.set_rate(
                &currency.0,
                &currency.1,
                bid.price as f64 / price_scale,
                ask.price as f64 / price_scale,
            );
            graph.set_symbol_for(&currency.0, &currency.1, snap.symbol.clone());
        }
    }

    fn apply_trade(books: &mut FxHashMap<SymbolId, BookSnapshot>, trade: TradePrint) {
        if let Some(book) = books.get_mut(&trade.symbol) {
            book.ts_exchange_ns = trade.ts_exchange_ns;
            book.ts_recv_ns = trade.ts_recv_ns;
        }
    }
}

fn parse_currency(symbol: &SymbolId) -> (String, String) {
    let s = &symbol.symbol;
    if s.ends_with("USDT") {
        let base = s.trim_end_matches("USDT").to_string();
        (base, "USDT".to_string())
    } else if s.ends_with("BTC") {
        let base = s.trim_end_matches("BTC").to_string();
        (base, "BTC".to_string())
    } else if s.ends_with("ETH") {
        let base = s.trim_end_matches("ETH").to_string();
        (base, "ETH".to_string())
    } else if s.ends_with("SOL") {
        let base = s.trim_end_matches("SOL").to_string();
        (base, "SOL".to_string())
    } else if s.ends_with("BNB") {
        let base = s.trim_end_matches("BNB").to_string();
        (base, "BNB".to_string())
    } else if s.ends_with("BUSD") {
        let base = s.trim_end_matches("BUSD").to_string();
        (base, "BUSD".to_string())
    } else if s.ends_with("FDUSD") {
        let base = s.trim_end_matches("FDUSD").to_string();
        (base, "FDUSD".to_string())
    } else {
        (s.clone(), "USDT".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_core::BookAction;
    use ta_core::OrderSide;

    /// Sets up book and graph data for a 3-currency triangle with a deliberate
    /// cross-rate mispricing:
    ///
    ///   BTCUSDT: bid = 100, ask = 101
    ///   ETHBTC:  bid =   1, ask =   2   (1 ETH = 1-2 BTC — cheap)
    ///   ETHUSDT: bid =   1, ask =   2   (1 ETH = 1-2 USDT — cheap)
    ///
    /// Arbitrage path: sell BTC→USDT (100), buy ETH→BTC (2), buy USDT→ETH (1)
    ///   → end with more USDT than you started.
    fn build_triangle_data(
        books: &mut FxHashMap<SymbolId, BookSnapshot>,
        graph: &mut ExchangeRateGraph,
    ) {
        let btcusdt = SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() };
        let ethbtc = SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() };
        let ethusdt = SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() };

        // BTCUSDT: bid = 100, ask = 101
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: btcusdt.clone(), side: Side::Bid, level: 0,
            price: 100, size: 1000, action: BookAction::Upsert,
            sequence: 1, ts_exchange_ns: 1, ts_recv_ns: 1,
        }, 1.0);
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: btcusdt, side: Side::Ask, level: 0,
            price: 101, size: 1000, action: BookAction::Upsert,
            sequence: 2, ts_exchange_ns: 2, ts_recv_ns: 2,
        }, 1.0);

        // ETHBTC: bid = 1, ask = 2
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: ethbtc.clone(), side: Side::Bid, level: 0,
            price: 1, size: 100_000, action: BookAction::Upsert,
            sequence: 3, ts_exchange_ns: 3, ts_recv_ns: 3,
        }, 1.0);
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: ethbtc.clone(), side: Side::Ask, level: 0,
            price: 2, size: 100_000, action: BookAction::Upsert,
            sequence: 4, ts_exchange_ns: 4, ts_recv_ns: 4,
        }, 1.0);

        // ETHUSDT: bid = 1, ask = 2
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: ethusdt.clone(), side: Side::Bid, level: 0,
            price: 1, size: 100_000, action: BookAction::Upsert,
            sequence: 5, ts_exchange_ns: 5, ts_recv_ns: 5,
        }, 1.0);
        FeedEngine::bench_apply_book_update(books, graph, BookUpdate {
            symbol: ethusdt.clone(), side: Side::Ask, level: 0,
            price: 2, size: 100_000, action: BookAction::Upsert,
            sequence: 6, ts_exchange_ns: 6, ts_recv_ns: 6,
        }, 1.0);
    }

    #[tokio::test]
    async fn test_pipeline_end_to_end() {
        let mut books = FxHashMap::default();
        let mut graph = ExchangeRateGraph::new();

        build_triangle_data(&mut books, &mut graph);

        // Verify graph has data
        assert_eq!(graph.currencies().len(), 3, "expected 3 currencies");
        assert!(graph.last_updated_at().elapsed().as_secs() < 1, "graph should be fresh");

        // Detection
        let detect = ta_detect::DetectionEngine::new(ta_detect::DetectionConfig {
            min_profit_bps: 0.01,
            fee_taker_bps: 0.0,
            max_data_age: std::time::Duration::from_secs(60),
            ..Default::default()
        });
        let opportunities = detect.detect(&graph);
        assert!(!opportunities.is_empty(), "expected at least one opportunity");

        let opp = &opportunities[0];
        assert_eq!(opp.routes.len(), 3, "expected 3 legs");
        for leg in &opp.routes {
            assert!(!leg.symbol.symbol.is_empty(), "symbol should not be empty");
            assert!(
                matches!(leg.side, OrderSide::Buy | OrderSide::Sell),
                "side should be Buy or Sell"
            );
        }
        assert!(opp.expected_profit_bps > 0.0, "profit should be positive");

        // Enrichment (simulate what enrich_opportunity does)
        let mut enriched = opp.clone();
        for leg in &mut enriched.routes {
            if let Some(book) = books.get(&leg.symbol) {
                match leg.side {
                    OrderSide::Buy => {
                        if let Some(ask) = book.asks.first() {
                            leg.price = ask.price;
                            leg.size = ask.size;
                        }
                    }
                    OrderSide::Sell => {
                        if let Some(bid) = book.bids.first() {
                            leg.price = bid.price;
                            leg.size = bid.size;
                        }
                    }
                }
            }
        }
        for leg in &enriched.routes {
            assert!(leg.price > 0, "price should be enriched");
            assert!(leg.size > 0, "size should be enriched");
        }

        // Execution
        let mut exec = ta_exec::ExecEngine::new(Default::default());
        let result = exec.execute_opportunity(&enriched, &books);
        // In mock mode execution will either succeed or fail gracefully
        // — the important thing is it doesn't panic
        tracing::debug!(?result, "execution result");
    }

    #[tokio::test]
    async fn test_pipeline_no_false_positive() {
        let mut books = FxHashMap::default();
        let mut graph = ExchangeRateGraph::new();

        // Consistent cross-rates — no arbitrage
        let btcusdt = SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() };
        let ethbtc = SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() };
        let ethusdt = SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() };

        // BTCUSDT: bid = 100, ask = 101
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: btcusdt.clone(), side: Side::Bid, level: 0,
            price: 100, size: 1000, action: BookAction::Upsert,
            sequence: 1, ts_exchange_ns: 1, ts_recv_ns: 1,
        }, 1.0);
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: btcusdt, side: Side::Ask, level: 0,
            price: 101, size: 1000, action: BookAction::Upsert,
            sequence: 2, ts_exchange_ns: 2, ts_recv_ns: 2,
        }, 1.0);

        // ETHBTC: bid = 50, ask = 51  → 1 ETH = 50-51 BTC
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: ethbtc.clone(), side: Side::Bid, level: 0,
            price: 50, size: 1000, action: BookAction::Upsert,
            sequence: 3, ts_exchange_ns: 3, ts_recv_ns: 3,
        }, 1.0);
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: ethbtc, side: Side::Ask, level: 0,
            price: 51, size: 1000, action: BookAction::Upsert,
            sequence: 4, ts_exchange_ns: 4, ts_recv_ns: 4,
        }, 1.0);

        // ETHUSDT: bid = 5000, ask = 5100  → 1 ETH = 5000-5100 USDT
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: ethusdt.clone(), side: Side::Bid, level: 0,
            price: 5000, size: 1000, action: BookAction::Upsert,
            sequence: 5, ts_exchange_ns: 5, ts_recv_ns: 5,
        }, 1.0);
        FeedEngine::bench_apply_book_update(&mut books, &mut graph, BookUpdate {
            symbol: ethusdt, side: Side::Ask, level: 0,
            price: 5100, size: 1000, action: BookAction::Upsert,
            sequence: 6, ts_exchange_ns: 6, ts_recv_ns: 6,
        }, 1.0);

        // Cross-rates are consistent: 50 BTC/ETH × 100 USDT/BTC = 5000 USDT/ETH
        let detect = ta_detect::DetectionEngine::new(ta_detect::DetectionConfig {
            min_profit_bps: 0.01,
            fee_taker_bps: 0.0,
            max_data_age: std::time::Duration::from_secs(60),
            ..Default::default()
        });
        let opportunities = detect.detect(&graph);
        assert!(
            opportunities.is_empty(),
            "consistent rates should not produce arbitrage, got {}",
            opportunities.len()
        );
    }

    #[test]
    fn test_parse_currency() {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "BTCUSDT".into(),
        };
        let (base, quote) = parse_currency(&symbol);
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USDT");
    }

    #[tokio::test]
    async fn test_book_update() {
        use rustc_hash::FxHashMap;
        let mut books = FxHashMap::default();
        let mut graph = ExchangeRateGraph::new();

        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "BTCUSDT".into(),
        };

        FeedEngine::apply_book_update(
            &mut books,
            BookUpdate {
                symbol: symbol.clone(),
                side: Side::Bid,
                level: 0,
                price: 50000_00_000_000,
                size: 100,
                action: BookAction::Upsert,
                sequence: 1,
                ts_exchange_ns: 1,
                ts_recv_ns: 2,
            },
            &mut graph,
            1_000_000.0,
        );

        FeedEngine::apply_book_update(
            &mut books,
            BookUpdate {
                symbol: symbol.clone(),
                side: Side::Ask,
                level: 0,
                price: 50001_00_000_000,
                size: 100,
                action: BookAction::Upsert,
                sequence: 2,
                ts_exchange_ns: 3,
                ts_recv_ns: 4,
            },
            &mut graph,
            1_000_000.0,
        );

        let snap = books.get(&symbol).unwrap();
        assert_eq!(snap.bids.len(), 1);
        assert_eq!(snap.asks.len(), 1);
        assert_eq!(snap.bids[0].price, 50000_00_000_000);
    }

    #[tokio::test]
    async fn test_feed_health_initial() {
        let feed = FeedEngine::new(None);
        let health = feed.health();
        assert!(!health.connected);
        assert!(health.last_message_at.is_none());
        assert_eq!(health.consecutive_errors, 0);
        assert!(health.degraded);
    }
}
