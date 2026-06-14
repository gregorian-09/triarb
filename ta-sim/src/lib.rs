use of_core::{BookLevel, BookSnapshot, SymbolId};
use std::collections::VecDeque;
use ta_core::OrderSide;

/// Result of simulating a market order fill against an order book.
#[derive(Debug, Clone)]
pub struct FillResult {
    pub fills: Vec<FillEvent>,
    pub avg_price: i64,
    pub total_qty: i64,
    pub slippage_bps: f64,
    pub fee_paid: i64,
}

/// A single fill event — a partial fill at a specific price level.
#[derive(Debug, Clone)]
pub struct FillEvent {
    pub price: i64,
    pub qty: i64,
}

/// Model for simulating market order fills with configurable slippage and fees.
#[derive(Debug, Clone)]
pub struct FillModel {
    /// Simulated latency in nanoseconds between decision and execution.
    pub latency_ns: u64,
    /// Taker fee in basis points applied to each fill.
    pub taker_fee_bps: f64,
    /// Slippage behaviour.
    pub slippage: SlippageModel,
}

#[derive(Debug, Clone)]
pub enum SlippageModel {
    /// Fill at top-of-book regardless of size (optimistic).
    None,
    /// Walk the book, consuming liquidity levels until the order is filled.
    Walk,
    /// Apply a fixed additional slippage in basis points on top of top-of-book.
    Fixed(f64),
}

impl Default for FillModel {
    fn default() -> Self {
        Self {
            latency_ns: 50_000, // 50µs
            taker_fee_bps: 10.0, // 10bps
            slippage: SlippageModel::Walk,
        }
    }
}

impl FillModel {
    /// Simulate executing a market order of `size` units on the given `side`
    /// against the current book snapshot.
    ///
    /// Returns the fills, average price, total filled quantity, slippage in bps,
    /// and total fee paid.
    pub fn execute(&self, side: OrderSide, size: i64, book: &BookSnapshot) -> FillResult {
        let levels: &[BookLevel] = match side {
            OrderSide::Buy => &book.asks,
            OrderSide::Sell => &book.bids,
        };

        let top_price = levels.first().map(|l| l.price).unwrap_or(0);
        let mut remaining = size;
        let mut fills = Vec::new();

        for level in levels {
            if remaining <= 0 {
                break;
            }
            let fill_qty = remaining.min(level.size);
            fills.push(FillEvent {
                price: level.price,
                qty: fill_qty,
            });
            remaining -= fill_qty;
        }

        let total_filled = size - remaining;
        if total_filled == 0 || fills.is_empty() {
            return FillResult {
                fills: Vec::new(),
                avg_price: 0,
                total_qty: 0,
                slippage_bps: 0.0,
                fee_paid: 0,
            };
        }

        let total_notional: i64 = fills.iter().map(|f| f.price.saturating_mul(f.qty)).sum();
        let avg_price = total_notional / total_filled;

        // Slippage in bps: how far the average price is from the top-of-book
        let slippage_bps = match side {
            OrderSide::Buy => {
                if top_price > 0 {
                    ((avg_price - top_price) as f64 / top_price as f64) * 10_000.0
                } else {
                    0.0
                }
            }
            OrderSide::Sell => {
                if top_price > 0 {
                    ((top_price - avg_price) as f64 / top_price as f64) * 10_000.0
                } else {
                    0.0
                }
            }
        };

        // Apply additional fixed slippage if configured
        let adjusted_avg = match (&self.slippage, side) {
            (SlippageModel::Fixed(bps), OrderSide::Buy) => {
                avg_price + (avg_price as f64 * bps / 10_000.0) as i64
            }
            (SlippageModel::Fixed(bps), OrderSide::Sell) => {
                avg_price - (avg_price as f64 * bps / 10_000.0) as i64
            }
            _ => avg_price,
        };

        let fee_paid = (adjusted_avg as f64 * total_filled as f64 * self.taker_fee_bps / 10_000.0)
            as i64;

        FillResult {
            fills,
            avg_price: adjusted_avg,
            total_qty: total_filled,
            slippage_bps,
            fee_paid,
        }
    }
}

#[derive(serde::Deserialize)]
pub struct RawTick {
    pub ts_ns: u64,
    pub venue: String,
    pub symbol: String,
    pub bid: i64,
    pub ask: i64,
    pub last_price: i64,
    pub last_size: i64,
}

pub struct Tick {
    pub ts_ns: u64,
    pub symbol: SymbolId,
    pub bid: i64,
    pub ask: i64,
    pub last_price: i64,
    pub last_size: i64,
}

pub struct SimulatedExchange {
    ticks: VecDeque<Tick>,
    current_idx: usize,
    books: std::collections::HashMap<SymbolId, BookSnapshot>,
}

impl RawTick {
    pub fn into_tick(self) -> Tick {
        Tick {
            ts_ns: self.ts_ns,
            symbol: SymbolId {
                venue: self.venue,
                symbol: self.symbol,
            },
            bid: self.bid,
            ask: self.ask,
            last_price: self.last_price,
            last_size: self.last_size,
        }
    }
}

impl SimulatedExchange {
    pub fn new(ticks: Vec<Tick>) -> Self {
        Self {
            ticks: VecDeque::from(ticks),
            current_idx: 0,
            books: std::collections::HashMap::new(),
        }
    }

    pub fn from_jsonl(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut ticks = VecDeque::new();

        for line in std::io::BufRead::lines(reader) {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let raw: RawTick = serde_json::from_str(&line)?;
            ticks.push_back(raw.into_tick());
        }

        Ok(Self {
            ticks,
            current_idx: 0,
            books: std::collections::HashMap::new(),
        })
    }

    pub fn advance(&mut self) -> Option<&Tick> {
        let tick = self.ticks.get(self.current_idx)?;
        self.current_idx += 1;

        let snap = self
            .books
            .entry(tick.symbol.clone())
            .or_insert_with(|| BookSnapshot {
                symbol: tick.symbol.clone(),
                bids: vec![BookLevel {
                    price: tick.bid,
                    size: 0,
                    level: 0,
                }],
                asks: vec![BookLevel {
                    price: tick.ask,
                    size: 0,
                    level: 0,
                }],
                last_sequence: self.current_idx as u64,
                ts_exchange_ns: tick.ts_ns,
                ts_recv_ns: tick.ts_ns,
            });

        if let Some(bid) = snap.bids.first_mut() {
            bid.price = tick.bid;
        }
        if let Some(ask) = snap.asks.first_mut() {
            ask.price = tick.ask;
        }
        snap.last_sequence = self.current_idx as u64;
        snap.ts_exchange_ns = tick.ts_ns;
        snap.ts_recv_ns = tick.ts_ns;

        Some(tick)
    }

    pub fn reset(&mut self) {
        self.current_idx = 0;
        self.books.clear();
    }

    pub fn len(&self) -> usize {
        self.ticks.len()
    }

    pub fn position(&self) -> usize {
        self.current_idx
    }

    pub fn book(&self, symbol: &SymbolId) -> Option<&BookSnapshot> {
        self.books.get(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_core::BookLevel;

    fn sample_book() -> BookSnapshot {
        BookSnapshot {
            symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() },
            bids: vec![
                BookLevel { price: 100_000, size: 10_000, level: 0 },
                BookLevel { price: 99_900, size: 20_000, level: 1 },
                BookLevel { price: 99_800, size: 30_000, level: 2 },
            ],
            asks: vec![
                BookLevel { price: 100_100, size: 10_000, level: 0 },
                BookLevel { price: 100_200, size: 20_000, level: 1 },
                BookLevel { price: 100_300, size: 30_000, level: 2 },
            ],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        }
    }

    #[test]
    fn test_fill_no_slippage_small_order() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Buy, 1_000, &book);
        assert_eq!(result.total_qty, 1_000);
        assert_eq!(result.avg_price, 100_100); // top of ask
        assert!(result.slippage_bps < 0.1);
    }

    #[test]
    fn test_fill_sell_no_slippage() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Sell, 5_000, &book);
        assert_eq!(result.total_qty, 5_000);
        assert_eq!(result.avg_price, 100_000); // top of bid
    }

    #[test]
    fn test_fill_walks_book() {
        let model = FillModel {
            slippage: SlippageModel::Walk,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        // 25_000 units: consumes 10k at level 0 + 15k at level 1
        let result = model.execute(OrderSide::Buy, 25_000, &book);
        assert_eq!(result.total_qty, 25_000);
        assert_eq!(result.fills.len(), 2);
        // Weighted average: (10k * 100100 + 15k * 100200) / 25k
        let expected_avg = (10_000i64 * 100_100 + 15_000 * 100_200) / 25_000;
        assert_eq!(result.avg_price, expected_avg);
        assert!(result.slippage_bps > 0.0);
    }

    #[test]
    fn test_fill_partial_exhausts_book() {
        let model = FillModel {
            slippage: SlippageModel::Walk,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        // 100_000 units: book only has 60k on ask side
        let result = model.execute(OrderSide::Buy, 100_000, &book);
        assert_eq!(result.total_qty, 60_000); // only what's available
        assert_eq!(result.fills.len(), 3);
    }

    #[test]
    fn test_fill_fees_applied() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 10.0, // 10bps
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Buy, 10_000, &book);
        assert_eq!(result.total_qty, 10_000);
        // 10bps of 100_100 * 10_000 = 10.01 * 10_000 = 100_100
        assert!(result.fee_paid > 0);
    }

    #[test]
    fn test_fill_empty_book() {
        let model = FillModel::default();
        let book = BookSnapshot {
            symbol: SymbolId { venue: "BINANCE".into(), symbol: "VOID".into() },
            bids: vec![],
            asks: vec![],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        };
        let result = model.execute(OrderSide::Buy, 100, &book);
        assert_eq!(result.total_qty, 0);
        assert!(result.fills.is_empty());
    }

    #[test]
    fn test_tick_advance() {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "BTCUSDT".into(),
        };

        let ticks = vec![
            Tick {
                ts_ns: 1,
                symbol: symbol.clone(),
                bid: 50000_00_000_000,
                ask: 50001_00_000_000,
                last_price: 50000_00_000_000,
                last_size: 100,
            },
            Tick {
                ts_ns: 2,
                symbol: symbol.clone(),
                bid: 50001_00_000_000,
                ask: 50002_00_000_000,
                last_price: 50001_00_000_000,
                last_size: 200,
            },
        ];

        let mut sim = SimulatedExchange::new(ticks);
        assert_eq!(sim.len(), 2);

        let t1 = sim.advance().unwrap();
        assert_eq!(t1.ts_ns, 1);

        let t2 = sim.advance().unwrap();
        assert_eq!(t2.ts_ns, 2);

        assert!(sim.advance().is_none());
    }

    #[test]
    fn test_reset() {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "ETHUSDT".into(),
        };

        let mut sim = SimulatedExchange::new(vec![Tick {
            ts_ns: 1,
            symbol,
            bid: 3000_00_000_000,
            ask: 3001_00_000_000,
            last_price: 3000_00_000_000,
            last_size: 10,
        }]);

        assert_eq!(sim.position(), 0);
        sim.advance();
        assert_eq!(sim.position(), 1);
        sim.reset();
        assert_eq!(sim.position(), 0);
    }
}
