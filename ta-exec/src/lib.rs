pub mod clock_skew;
mod hedge;
mod journal;
mod order_timeout;
mod price_check;

pub use clock_skew::*;
pub use hedge::*;
pub use journal::*;
pub use order_timeout::*;
pub use price_check::*;

use of_core::{BookSnapshot, SymbolId};
use of_execution_core::{AccountId, ExecutionSymbol, RouteId};
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use ta_core::{ArbitrageOpportunity, DedupTable, FillState, OpportunityId};

pub struct ExecConfig {
    pub route_id: RouteId,
    pub account_id: AccountId,
    pub symbol: ExecutionSymbol,
    pub dedup_ttl: Duration,
    /// Path to the order journal JSONL file. None = no persistence.
    pub journal_path: Option<PathBuf>,
    pub price_tolerance: PriceTolerance,
    pub order_timeout: Duration,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            route_id: RouteId::new("BINANCE").unwrap(),
            account_id: AccountId::new("MAIN").unwrap(),
            symbol: ExecutionSymbol::new("BINANCE", "BTCUSDT").unwrap(),
            dedup_ttl: Duration::from_secs(60),
            journal_path: None,
            price_tolerance: PriceTolerance::default(),
            order_timeout: Duration::from_secs(5),
        }
    }
}

pub struct ExecEngine {
    dedup: DedupTable,
    pending_fills: HashMap<OpportunityId, FillState>,
    journal: Option<FileOrderLog>,
    price_checker: PriceChecker,
    gc_counter: u64,
    order_tracker: OrderTimeoutTracker,
}

impl ExecEngine {
    pub fn new(config: ExecConfig) -> Self {
        let journal = config.journal_path.map(|path| {
            match FileOrderLog::open(&path) {
                Ok(log) => {
                    log.report_unacknowledged();
                    log
                }
                Err(e) => {
                    tracing::error!("failed to open journal at {path:?}: {e}");
                    FileOrderLog::open(&path).expect("journal must be openable")
                }
            }
        });

        Self {
            dedup: DedupTable::new(config.dedup_ttl),
            pending_fills: HashMap::new(),
            journal,
            price_checker: PriceChecker::new(config.price_tolerance),
            gc_counter: 0,
            order_tracker: OrderTimeoutTracker::new(OrderTimeoutConfig {
                submission_timeout: config.order_timeout,
            }),
        }
    }

    /// Attempt to execute an arbitrage opportunity.
    ///
    /// `books` is the current book snapshot map from the feed engine,
    /// used for pre-submission price validation.
    pub fn execute_opportunity(
        &mut self,
        opp: &ArbitrageOpportunity,
        books: &FxHashMap<SymbolId, BookSnapshot>,
    ) -> Result<OpportunityResult, ExecError> {
        let opp_id = OpportunityId::from_opportunity(opp);

        // 1. Dedup check
        if !self.dedup.check_and_insert(opp_id) {
            tracing::debug!(?opp_id, "duplicate opportunity skipped");
            return Ok(OpportunityResult::Duplicate);
        }

        self.periodic_gc();

        // 2. Pre-submission price check
        if let Err(failure) = self.price_checker.check_opportunity(&opp.routes, books) {
            tracing::warn!(?opp_id, error = %failure, "price check failed, aborting");
            return Ok(OpportunityResult::PriceCheckFailed(
                failure.reason.clone(),
            ));
        }

        // 3. Execute each leg
        let mut state = FillState::new(opp_id);
        let leg_count = opp.routes.len().min(3);

        for leg_idx in 0..leg_count {
            let leg = &opp.routes[leg_idx];

            // Record intent before submission
            if let Some(ref mut log) = self.journal {
                let venue = &leg.symbol.venue;
                let symbol = &leg.symbol.symbol;
                let side = match leg.side {
                    ta_core::OrderSide::Buy => "Buy",
                    ta_core::OrderSide::Sell => "Sell",
                };
                log.record_intent(&opp_id, venue, symbol, side);
            }

            match self.submit_leg(opp, leg_idx) {
                Ok(()) => {
                    state.fill_leg(leg_idx);
                    self.order_tracker.resolve_leg(&opp_id, leg_idx);
                    if let Some(ref mut log) = self.journal {
                        log.record_fill(
                            &opp_id,
                            leg_idx,
                            &leg.symbol.venue,
                            &leg.symbol.symbol,
                            leg.price,
                            leg.size,
                        );
                    }
                }
                Err(e) => {
                    state.fail_leg(leg_idx);
                    self.order_tracker.resolve_leg(&opp_id, leg_idx);
                    if let Some(ref mut log) = self.journal {
                        log.record_fail(&opp_id, leg_idx, &e.to_string());
                    }
                    tracing::error!(?opp_id, leg = leg_idx, error = %e, "leg submission failed");
                    break;
                }
            }
        }

        // 4. Handle partial fill → rollback
        if state.needs_rollback() {
            tracing::warn!(?opp_id, "partial fill detected, initiating rollback");
            for leg_idx in 0..leg_count {
                if state.legs[leg_idx] == ta_core::LegFillStatus::Filled {
                    self.hedge_leg(opp, leg_idx);
                    if let Some(ref mut log) = self.journal {
                        let leg = &opp.routes[leg_idx];
                        log.record_hedge(
                            &opp_id,
                            leg_idx,
                            &leg.symbol.venue,
                            &leg.symbol.symbol,
                            0, // hedge price TBD
                        );
                    }
                }
            }
            self.pending_fills.insert(opp_id, state.clone());
            self.order_tracker.resolve_opportunity(&opp_id);
            return Ok(OpportunityResult::RolledBack(state));
        }

        // 5. Report success
        if state.is_fully_filled() {
            self.order_tracker.resolve_opportunity(&opp_id);
            tracing::info!(?opp_id, profit_bps = opp.expected_profit_bps, "opportunity fully filled");
            return Ok(OpportunityResult::Filled(state));
        }

        self.order_tracker.resolve_opportunity(&opp_id);
        Ok(OpportunityResult::Failed)
    }

    fn submit_leg(&mut self, opp: &ArbitrageOpportunity, leg_idx: usize) -> Result<(), ExecError> {
        let opp_id = OpportunityId::from_opportunity(opp);
        self.order_tracker.record_submission(opp_id, leg_idx);
        Ok(())
    }

    fn hedge_leg(&self, opp: &ArbitrageOpportunity, leg_idx: usize) {
        match hedge_spec(opp, leg_idx) {
            Some(spec) => {
                tracing::warn!(
                    leg = leg_idx,
                    hedge_symbol = %spec.symbol.symbol,
                    hedge_side = ?spec.side,
                    hedge_qty = spec.size,
                    "hedging filled leg"
                );
                if let Err(e) = submit_hedge(&spec) {
                    tracing::error!(leg = leg_idx, error = %e, "hedge submission failed");
                }
            }
            None => {
                tracing::error!(leg = leg_idx, "cannot build hedge spec: leg out of range");
            }
        }
    }

    /// Check for timed-out orders and return any that need attention.
    pub fn check_order_timeouts(&mut self) -> Vec<(OpportunityId, usize)> {
        let timeouts = self.order_tracker.check_timeouts();
        if !timeouts.is_empty() {
            tracing::warn!("{} order(s) timed out", timeouts.len());
            for (opp_id, leg_idx) in &timeouts {
                tracing::warn!(?opp_id, leg = leg_idx, "order timed out");
                self.order_tracker.resolve_leg(opp_id, *leg_idx);
            }
        }
        timeouts
    }

    fn periodic_gc(&mut self) {
        self.gc_counter += 1;
        if self.gc_counter % 100 == 0 {
            self.dedup.gc();
        }
    }
}

#[derive(Debug)]
pub enum ExecError {
    SubmissionFailed(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::SubmissionFailed(msg) => write!(f, "submission failed: {msg}"),
        }
    }
}

#[derive(Debug)]
pub enum OpportunityResult {
    Duplicate,
    Filled(FillState),
    RolledBack(FillState),
    TimedOut(FillState),
    Failed,
    PriceCheckFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ta_core::{Triangle, RouteLeg, OrderSide, of_core::SymbolId};

    fn dummy_opportunity() -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: ("USDT".into(), "BTC".into()),
                leg_b: ("BTC".into(), "ETH".into()),
                leg_c: ("ETH".into(), "USDT".into()),
                opportunity_bps: 15.0,
            },
            routes: vec![
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() }, side: OrderSide::Buy, price: 50000_00_000_000, size: 100 },
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() }, side: OrderSide::Sell, price: 5000_00_000_000, size: 100 },
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() }, side: OrderSide::Sell, price: 3000_00_000_000, size: 100 },
            ],
            expected_profit_bps: 5.0,
            ts_ns: 0,
        }
    }

    fn dummy_books() -> FxHashMap<SymbolId, BookSnapshot> {
        use of_core::BookLevel;
        let mut books = FxHashMap::default();
        let mk = |sym: &str, bid: i64, ask: i64| BookSnapshot {
            symbol: SymbolId { venue: "BINANCE".into(), symbol: sym.into() },
            bids: vec![BookLevel { price: bid, size: 100_000, level: 0 }],
            asks: vec![BookLevel { price: ask, size: 100_000, level: 0 }],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        };
        books.insert(SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() }, mk("BTCUSDT", 50000_00_000_000, 50001_00_000_000));
        books.insert(SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() }, mk("ETHBTC", 5000_00_000_000, 5001_00_000_000));
        books.insert(SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() }, mk("ETHUSDT", 3000_00_000_000, 3001_00_000_000));
        books
    }

    #[test]
    fn test_dedup_rejects_duplicate() {
        let mut engine = ExecEngine::new(ExecConfig::default());
        let opp = dummy_opportunity();
        let books = dummy_books();
        let r1 = engine.execute_opportunity(&opp, &books).unwrap();
        let r2 = engine.execute_opportunity(&opp, &books).unwrap();
        assert!(matches!(r1, OpportunityResult::Filled(_)));
        assert!(matches!(r2, OpportunityResult::Duplicate));
    }

    #[test]
    fn test_price_check_rejects_stale() {
        let mut engine = ExecEngine::new(ExecConfig::default());
        let mut opp = dummy_opportunity();
        // Set a high tight price that won't match the book
        opp.routes[0].price = 99999_00_000_000;
        let books = dummy_books();
        let result = engine.execute_opportunity(&opp, &books).unwrap();
        assert!(matches!(result, OpportunityResult::PriceCheckFailed(_)));
    }

    #[test]
    fn test_exec_engine_creation() {
        let mut engine = ExecEngine::new(ExecConfig::default());
        let opp = dummy_opportunity();
        let books = dummy_books();
        let result = engine.execute_opportunity(&opp, &books).unwrap();
        assert!(matches!(result, OpportunityResult::Filled(_)));
    }
}
