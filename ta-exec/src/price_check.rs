use of_core::{BookSnapshot, SymbolId};
use ta_core::RouteLeg;

/// Slippage tolerance for pre-submission price validation.
#[derive(Debug, Clone, Copy)]
pub struct PriceTolerance {
    /// Maximum allowed slippage in basis points from the detected price.
    pub max_slippage_bps: f64,
}

impl Default for PriceTolerance {
    fn default() -> Self {
        Self {
            max_slippage_bps: 2.0, // 0.02% — very tight for arb
        }
    }
}

/// Checks that current market conditions still support a detected arb leg.
pub struct PriceChecker {
    pub tolerance: PriceTolerance,
}

impl Default for PriceChecker {
    fn default() -> Self {
        Self {
            tolerance: PriceTolerance::default(),
        }
    }
}

impl PriceChecker {
    pub fn new(tolerance: PriceTolerance) -> Self {
        Self { tolerance }
    }

    /// Returns `Ok(())` if the current book still supports the expected leg price.
    ///
    /// For a Buy leg: the current ask must be within slippage of the expected price.
    /// For a Sell leg: the current bid must be within slippage of the expected price.
    /// Returns an error with a description if the check fails.
    pub fn check_leg(
        &self,
        leg: &RouteLeg,
        expected_price: i64,
        book: &BookSnapshot,
    ) -> Result<(), PriceCheckFailure> {
        let top = match leg.side {
            ta_core::OrderSide::Buy => book.asks.first(),
            ta_core::OrderSide::Sell => book.bids.first(),
        };

        let top = top.ok_or_else(|| PriceCheckFailure {
            reason: format!(
                "no {} side liquidity for {}",
                match leg.side {
                    ta_core::OrderSide::Buy => "ask",
                    ta_core::OrderSide::Sell => "bid",
                },
                leg.symbol.symbol
            ),
        })?;

        let current_price = top.price;
        let diff = (current_price - expected_price).abs() as f64;
        let slippage_bps = diff / expected_price as f64 * 10_000.0;

        if slippage_bps > self.tolerance.max_slippage_bps {
            return Err(PriceCheckFailure {
                reason: format!(
                    "slippage {:.2} bps exceeds tolerance {:.1} bps for {} (expected {}, current {})",
                    slippage_bps,
                    self.tolerance.max_slippage_bps,
                    leg.symbol.symbol,
                    expected_price,
                    current_price,
                ),
            });
        }

        Ok(())
    }

    /// Check all three legs of an opportunity. Returns the first failure.
    pub fn check_opportunity(
        &self,
        legs: &[RouteLeg],
        books: &rustc_hash::FxHashMap<SymbolId, BookSnapshot>,
    ) -> Result<(), PriceCheckFailure> {
        for leg in legs {
            let book = books.get(&leg.symbol).ok_or_else(|| PriceCheckFailure {
                reason: format!("no book data for {}", leg.symbol.symbol),
            })?;
            self.check_leg(leg, leg.price, book)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct PriceCheckFailure {
    pub reason: String,
}

impl std::fmt::Display for PriceCheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "price check failed: {}", self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_core::{BookLevel, BookSnapshot, SymbolId};
    use ta_core::{OrderSide, RouteLeg};

    fn make_book(bid_price: i64, ask_price: i64) -> BookSnapshot {
        BookSnapshot {
            symbol: SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
            bids: vec![BookLevel {
                price: bid_price,
                size: 100_000,
                level: 0,
            }],
            asks: vec![BookLevel {
                price: ask_price,
                size: 100_000,
                level: 0,
            }],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        }
    }

    #[test]
    fn test_price_check_accepts_good_price() {
        let checker = PriceChecker::default();
        let leg = RouteLeg {
            symbol: SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
            side: OrderSide::Buy,
            price: 50000_00_000_000,
            size: 100,
        };
        let book = make_book(50000_00_000_000, 50001_00_000_000);
        assert!(checker.check_leg(&leg, leg.price, &book).is_ok());
    }

    #[test]
    fn test_price_check_rejects_excessive_slippage() {
        let checker = PriceChecker::default();
        let leg = RouteLeg {
            symbol: SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
            side: OrderSide::Buy,
            price: 50000_00_000_000,
            size: 100,
        };
        // Ask moved way above expected
        let book = make_book(50000_00_000_000, 50100_00_000_000);
        assert!(checker.check_leg(&leg, leg.price, &book).is_err());
    }

    #[test]
    fn test_price_check_no_liquidity() {
        let checker = PriceChecker::default();
        let leg = RouteLeg {
            symbol: SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
            side: OrderSide::Sell,
            price: 50000_00_000_000,
            size: 100,
        };
        let book = BookSnapshot {
            symbol: SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
            bids: vec![], // no bids
            asks: vec![BookLevel {
                price: 50001_00_000_000,
                size: 100,
                level: 0,
            }],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        };
        assert!(checker.check_leg(&leg, leg.price, &book).is_err());
    }
}
