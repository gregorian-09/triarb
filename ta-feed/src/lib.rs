use of_adapters::{AdapterConfig, MarketDataAdapter, ProviderKind, RawEvent, SubscribeReq};
use of_core::{BookLevel, BookSnapshot, BookUpdate, Side, SymbolId, TradePrint};
use rustc_hash::FxHashMap;
use std::sync::Arc;
use ta_core::ExchangeRateGraph;
use tokio::sync::RwLock;

const TOP_DEPTH: u16 = 10;

pub struct FeedEngine {
    adapter: Box<dyn MarketDataAdapter + Send>,
    books: Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>>,
    graph: Arc<RwLock<ExchangeRateGraph>>,
    _subscribed: Vec<SymbolId>,
}

impl FeedEngine {
    pub fn new(endpoint: Option<String>) -> Self {
        let cfg = AdapterConfig {
            provider: ProviderKind::Binance,
            endpoint,
            ..Default::default()
        };
        let adapter = of_adapters::create_adapter(&cfg).expect("create Binance adapter");
        Self {
            adapter,
            books: Arc::new(RwLock::new(FxHashMap::default())),
            graph: Arc::new(RwLock::new(ExchangeRateGraph::new())),
            _subscribed: Vec::new(),
        }
    }

    pub fn book_reader(&self) -> Arc<RwLock<FxHashMap<SymbolId, BookSnapshot>>> {
        self.books.clone()
    }

    pub fn graph_reader(&self) -> Arc<RwLock<ExchangeRateGraph>> {
        self.graph.clone()
    }

    pub async fn connect(&mut self) {
        self.adapter.connect().expect("connect Binance adapter");
    }

    pub async fn subscribe(&mut self, symbol: SymbolId) {
        self.adapter
            .subscribe(SubscribeReq {
                symbol: symbol.clone(),
                depth_levels: TOP_DEPTH,
            })
            .expect("subscribe");
        self._subscribed.push(symbol);
    }

    pub async fn poll(&mut self) {
        let mut events = Vec::new();
        match self.adapter.poll(&mut events) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("poll error: {e}");
                return;
            }
        }

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
                if let Some(existing) = levels.iter_mut().find(|l: &&mut BookLevel| l.level == update.level) {
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
        }
    }

    fn apply_trade(_books: &mut FxHashMap<SymbolId, BookSnapshot>, _trade: TradePrint) {}
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
}
