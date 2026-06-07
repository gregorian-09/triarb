use of_execution::{simulated_engine, ExecutionEngine, ExecutionEventBuffer, RouteConfig};
use of_execution::InMemoryJournal;
use of_execution::SimExecutionAdapter;
use of_execution_core::{AccountId, ExecutionSymbol, RiskLimits, RouteId};
use ta_core::{ArbitrageOpportunity, DedupTable, FillState, OpportunityId};
use std::collections::HashMap;
use std::time::Duration;

pub struct ExecConfig {
    pub route_id: RouteId,
    pub account_id: AccountId,
    pub symbol: ExecutionSymbol,
    /// TTL for dedup entries (default 60s).
    pub dedup_ttl: Duration,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            route_id: RouteId::new("BINANCE").unwrap(),
            account_id: AccountId::new("MAIN").unwrap(),
            symbol: ExecutionSymbol::new("BINANCE", "BTCUSDT").unwrap(),
            dedup_ttl: Duration::from_secs(60),
        }
    }
}

pub struct ExecEngine {
    engine: ExecutionEngine<SimExecutionAdapter, of_execution_core::BasicRiskGate, InMemoryJournal>,
    buf: ExecutionEventBuffer,
    dedup: DedupTable,
    pending_fills: HashMap<OpportunityId, FillState>,
    gc_counter: u64,
}

impl ExecEngine {
    pub fn new(config: ExecConfig) -> Self {
        let route = RouteConfig {
            route_id: config.route_id,
            account_id: config.account_id,
            symbol: config.symbol,
            enabled: true,
            risk_limits: RiskLimits {
                kill_switch: false,
                max_order_qty: 100,
                max_order_notional: 1_000_000,
                max_open_orders: 10,
                max_open_notional: 10_000_000,
                price_band_ticks: 0,
            },
        };
        let mut engine = simulated_engine(route);
        engine.start().unwrap();
        Self {
            engine,
            buf: ExecutionEventBuffer::with_capacity(64),
            dedup: DedupTable::new(config.dedup_ttl),
            pending_fills: HashMap::new(),
            gc_counter: 0,
        }
    }

    /// Attempt to execute an arbitrage opportunity.
    ///
    /// Returns `Ok(true)` if submitted, `Ok(false)` if it was a duplicate.
    pub fn execute_opportunity(&mut self, opp: &ArbitrageOpportunity) -> Result<OpportunityResult, ExecError> {
        let opp_id = OpportunityId::from_opportunity(opp);
        if !self.dedup.check_and_insert(opp_id) {
            tracing::debug!(?opp_id, "duplicate opportunity skipped");
            return Ok(OpportunityResult::Duplicate);
        }

        self.periodic_gc();

        let mut state = FillState::new(opp_id);
        let leg_count = opp.routes.len().min(3);

        for leg_idx in 0..leg_count {
            match self.submit_leg(opp, leg_idx) {
                Ok(()) => state.fill_leg(leg_idx),
                Err(e) => {
                    state.fail_leg(leg_idx);
                    tracing::error!(?opp_id, leg = leg_idx, error = %e, "leg submission failed");
                    break;
                }
            }
        }

        if state.needs_rollback() {
            tracing::warn!(?opp_id, "partial fill detected, initiating rollback");
            for leg_idx in 0..leg_count {
                if state.legs[leg_idx] == ta_core::LegFillStatus::Filled {
                    self.hedge_leg(opp, leg_idx);
                }
            }
            self.pending_fills.insert(opp_id, state.clone());
            return Ok(OpportunityResult::RolledBack(state));
        }

        if state.is_fully_filled() {
            tracing::info!(?opp_id, profit_bps = opp.expected_profit_bps, "opportunity fully filled");
            return Ok(OpportunityResult::Filled(state));
        }

        Ok(OpportunityResult::Failed)
    }

    fn submit_leg(&self, _opp: &ArbitrageOpportunity, _leg_idx: usize) -> Result<(), ExecError> {
        // TODO: real order submission via of_execution::ExecutionEngine
        Ok(())
    }

    fn hedge_leg(&self, _opp: &ArbitrageOpportunity, _leg_idx: usize) {
        // TODO: hedge filled leg on spot market
        tracing::warn!(leg = _leg_idx, "hedging filled leg (not yet implemented)");
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
    Failed,
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
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() }, side: OrderSide::Buy, price: 0, size: 0 },
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() }, side: OrderSide::Sell, price: 0, size: 0 },
                RouteLeg { symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() }, side: OrderSide::Sell, price: 0, size: 0 },
            ],
            expected_profit_bps: 5.0,
            ts_ns: 0,
        }
    }

    #[test]
    fn test_dedup_rejects_duplicate() {
        let mut engine = ExecEngine::new(ExecConfig::default());
        let opp = dummy_opportunity();
        let r1 = engine.execute_opportunity(&opp).unwrap();
        let r2 = engine.execute_opportunity(&opp).unwrap();
        assert!(matches!(r1, OpportunityResult::Filled(_)));
        assert!(matches!(r2, OpportunityResult::Duplicate));
    }

    #[test]
    fn test_exec_engine_creation() {
        let engine = ExecEngine::new(ExecConfig::default());
        assert!(engine.engine.health().connected);
    }
}
