use of_core::{BookLevel, BookSnapshot, SymbolId};
use std::collections::VecDeque;

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
