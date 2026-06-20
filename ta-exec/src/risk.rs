use std::collections::HashMap;
use std::time::{Duration, Instant};

use of_core::SymbolId;

/// High-level risk controls on top of `of_execution::RiskLimits`.
///
/// Tracks:
/// - Circuit breaker: pause after N consecutive failures
/// - Daily trade cap: max trades per rolling 24h window
/// - Per-symbol notional limits
#[derive(Debug, Clone)]
pub struct RiskConfig {
    /// Max consecutive execution failures before circuit breaker trips.
    pub max_consecutive_failures: u32,
    /// Duration the circuit breaker stays open.
    pub circuit_breaker_cooldown: Duration,
    /// Max trades in the rolling window.
    pub max_trades_per_window: u32,
    /// Rolling window duration.
    pub trade_window: Duration,
    /// Max notional (in quote currency) per symbol across open positions.
    pub max_notional_per_symbol: i64,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_consecutive_failures: 5,
            circuit_breaker_cooldown: Duration::from_secs(60),
            max_trades_per_window: 1000,
            trade_window: Duration::from_secs(86400),
            max_notional_per_symbol: 10_000_000_000_000_000,
        }
    }
}

#[derive(Debug)]
pub enum RiskRejection {
    CircuitBreakerActive {
        remaining_secs: u64,
    },
    DailyTradeCapExceeded {
        trades: u32,
        max: u32,
    },
    MaxNotionalExceeded {
        symbol: SymbolId,
        current: i64,
        max: i64,
    },
}

impl std::fmt::Display for RiskRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskRejection::CircuitBreakerActive { remaining_secs } => {
                write!(f, "circuit breaker active for {remaining_secs}s")
            }
            RiskRejection::DailyTradeCapExceeded { trades, max } => {
                write!(f, "daily trade cap exceeded: {trades}/{max}")
            }
            RiskRejection::MaxNotionalExceeded {
                symbol,
                current,
                max,
            } => {
                write!(f, "max notional exceeded for {symbol:?}: {current}/{max}")
            }
        }
    }
}

pub struct RiskController {
    config: RiskConfig,
    consecutive_failures: u32,
    circuit_breaker_until: Option<Instant>,
    trade_timestamps: Vec<Instant>,
    symbol_notional: HashMap<SymbolId, i64>,
}

impl RiskController {
    pub fn new(config: RiskConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
            circuit_breaker_until: None,
            trade_timestamps: Vec::new(),
            symbol_notional: HashMap::new(),
        }
    }

    pub fn check(&self) -> Result<(), RiskRejection> {
        // Circuit breaker
        if let Some(until) = self.circuit_breaker_until {
            if Instant::now() < until {
                return Err(RiskRejection::CircuitBreakerActive {
                    remaining_secs: until.duration_since(Instant::now()).as_secs(),
                });
            }
        }

        // Daily trade cap
        let cutoff = Instant::now() - self.config.trade_window;
        let recent_trades = self
            .trade_timestamps
            .iter()
            .filter(|t| **t > cutoff)
            .count() as u32;
        if recent_trades >= self.config.max_trades_per_window {
            return Err(RiskRejection::DailyTradeCapExceeded {
                trades: recent_trades,
                max: self.config.max_trades_per_window,
            });
        }

        Ok(())
    }

    pub fn check_symbol_notional(
        &self,
        symbol: &SymbolId,
        additional: i64,
    ) -> Result<(), RiskRejection> {
        let current = self.symbol_notional.get(symbol).copied().unwrap_or(0);
        if current + additional > self.config.max_notional_per_symbol {
            return Err(RiskRejection::MaxNotionalExceeded {
                symbol: symbol.clone(),
                current: current + additional,
                max: self.config.max_notional_per_symbol,
            });
        }
        Ok(())
    }

    pub fn record_success(&mut self, trade_size: i64, symbol: &SymbolId) {
        self.consecutive_failures = 0;
        self.trade_timestamps.push(Instant::now());
        *self.symbol_notional.entry(symbol.clone()).or_insert(0) += trade_size;
        self.gc();
    }

    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.config.max_consecutive_failures {
            self.circuit_breaker_until =
                Some(Instant::now() + self.config.circuit_breaker_cooldown);
            tracing::warn!(
                failures = self.consecutive_failures,
                cooldown_secs = self.config.circuit_breaker_cooldown.as_secs(),
                "circuit breaker tripped"
            );
        }
    }

    fn gc(&mut self) {
        let cutoff = Instant::now() - self.config.trade_window;
        self.trade_timestamps.retain(|t| *t > cutoff);
        // Notional GC: reset periodically — positions are assumed short-lived for arb
        if self.trade_timestamps.len() > self.config.max_trades_per_window as usize * 2 {
            self.symbol_notional.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_controller_allows_normal() {
        let rc = RiskController::new(RiskConfig::default());
        assert!(rc.check().is_ok());
    }

    #[test]
    fn test_circuit_breaker_trips_after_n_failures() {
        let mut rc = RiskController::new(RiskConfig {
            max_consecutive_failures: 3,
            circuit_breaker_cooldown: Duration::from_secs(10),
            ..Default::default()
        });
        for _ in 0..3 {
            rc.record_failure();
        }
        assert!(rc.check().is_err());
        assert!(matches!(
            rc.check(),
            Err(RiskRejection::CircuitBreakerActive { .. })
        ));
    }

    #[test]
    fn test_circuit_breaker_resets_after_success() {
        let mut rc = RiskController::new(RiskConfig {
            max_consecutive_failures: 3,
            ..Default::default()
        });
        rc.record_failure();
        rc.record_failure();
        rc.record_success(
            100,
            &SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
        );
        assert!(
            rc.check().is_ok(),
            "success should reset consecutive failures"
        );
    }

    #[test]
    fn test_daily_trade_cap() {
        let mut rc = RiskController::new(RiskConfig {
            max_trades_per_window: 5,
            trade_window: Duration::from_secs(3600),
            ..Default::default()
        });
        for _ in 0..5 {
            rc.record_success(
                100,
                &SymbolId {
                    venue: "BINANCE".into(),
                    symbol: "BTCUSDT".into(),
                },
            );
        }
        assert!(rc.check().is_err());
        assert!(matches!(
            rc.check(),
            Err(RiskRejection::DailyTradeCapExceeded { .. })
        ));
    }

    #[test]
    fn test_symbol_notional_limit() {
        let mut rc = RiskController::new(RiskConfig {
            max_notional_per_symbol: 1000,
            ..Default::default()
        });
        let sym = SymbolId {
            venue: "BINANCE".into(),
            symbol: "BTCUSDT".into(),
        };
        assert!(rc.check_symbol_notional(&sym, 500).is_ok());
        rc.record_success(500, &sym);
        // Now the running total is 500, so 600 would exceed the 1000 limit
        assert!(rc.check_symbol_notional(&sym, 600).is_err());
        // But 400 would still be within the budget (500+400=900 < 1000)
        assert!(rc.check_symbol_notional(&sym, 400).is_ok());
    }
}
