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
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            message_timeout: Duration::from_secs(10),
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

pub struct FeedEngine {
    adapter: Box<dyn MarketDataAdapter + Send>,
    books: Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>>,
    graph: Arc<RwLock<ExchangeRateGraph>>,
    _subscribed: Vec<SymbolId>,
    connected: bool,
    last_message_at: Option<Instant>,
    consecutive_errors: u32,
    config: FeedConfig,
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
            _subscribed: Vec::new(),
            connected: false,
            last_message_at: None,
            consecutive_errors: 0,
            config,
        }
    }

    pub fn book_reader(&self) -> Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>> {
        self.books.clone()
    }

    pub fn graph_reader(&self) -> Arc<RwLock<ExchangeRateGraph>> {
        self.graph.clone()
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
                tracing::info!("feed connected");
            }
            Err(e) => {
                self.connected = false;
                self.consecutive_errors += 1;
                tracing::error!("feed connect failed: {e}");
            }
        }
    }

    pub async fn subscribe(&mut self, symbol: SymbolId) {
        match self.adapter.subscribe(SubscribeReq {
            symbol: symbol.clone(),
            depth_levels: TOP_DEPTH,
        }) {
            Ok(_) => {
                self._subscribed.push(symbol);
            }
            Err(e) => {
                self.consecutive_errors += 1;
                tracing::error!("feed subscribe failed: {e}");
            }
        }
    }

    pub async fn poll(&mut self) {
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
                RawEvent::Book(update) => {
                    Self::apply_book_update(&mut books, update, &mut graph);
                }
                RawEvent::Trade(trade) => {
                    Self::apply_trade(&mut books, trade);
                }
            }
        }
    }

    pub fn bench_apply_book_update(
        books: &mut FxHashMap<SymbolId, BookSnapshot>,
        graph: &mut ExchangeRateGraph,
        update: BookUpdate,
    ) {
        FeedEngine::apply_book_update(books, update, graph);
    }

    fn apply_book_update(
        books: &mut FxHashMap<SymbolId, BookSnapshot>,
        update: BookUpdate,
        graph: &mut ExchangeRateGraph,
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
            graph.set_rate(&currency.0, &currency.1, bid.price, ask.price);
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
