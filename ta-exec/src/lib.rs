use of_execution::{simulated_engine, ExecutionEngine, ExecutionEventBuffer, RouteConfig};
use of_execution::InMemoryJournal;
use of_execution::SimExecutionAdapter;
use of_execution_core::{AccountId, ExecutionSymbol, RiskLimits, RouteId};
use ta_core::ArbitrageOpportunity;

pub struct ExecConfig {
    pub route_id: RouteId,
    pub account_id: AccountId,
    pub symbol: ExecutionSymbol,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            route_id: RouteId::new("BINANCE").unwrap(),
            account_id: AccountId::new("MAIN").unwrap(),
            symbol: ExecutionSymbol::new("BINANCE", "BTCUSDT").unwrap(),
        }
    }
}

pub struct ExecEngine {
    engine: ExecutionEngine<SimExecutionAdapter, of_execution_core::BasicRiskGate, InMemoryJournal>,
    buf: ExecutionEventBuffer,
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
        }
    }

    pub fn execute_opportunity(&mut self, opp: &ArbitrageOpportunity) {
        let _ = opp;
        tracing::info!("executing arbitrage opportunity");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_engine_creation() {
        let engine = ExecEngine::new(ExecConfig::default());
        assert!(engine.engine.health().connected);
    }
}
