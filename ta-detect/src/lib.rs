use rustc_hash::FxHashMap;
use ta_core::{ArbitrageOpportunity, Currency, ExchangeRateGraph, RouteLeg, Triangle};

pub struct DetectionConfig {
    pub min_profit_bps: f64,
    pub max_legs: usize,
    pub fee_taker_bps: f64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            min_profit_bps: 10.0,
            max_legs: 3,
            fee_taker_bps: 10.0,
        }
    }
}

pub struct DetectionEngine {
    config: DetectionConfig,
    _triangle_cache: FxHashMap<(Currency, Currency, Currency), Triangle>,
}

impl DetectionEngine {
    pub fn new(config: DetectionConfig) -> Self {
        Self {
            config,
            _triangle_cache: FxHashMap::default(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(DetectionConfig::default())
    }

    pub fn detect(&self, graph: &ExchangeRateGraph) -> Vec<ArbitrageOpportunity> {
        let raw = graph.detect();
        let mut opportunities = Vec::new();

        for (cycle, raw_profit) in raw {
            let profit_bps = raw_profit * 10_000.0;
            let fee_threshold = self.config.fee_taker_bps * 3.0;
            if profit_bps < self.config.min_profit_bps + fee_threshold {
                continue;
            }

            if cycle.len() > self.config.max_legs + 1 {
                continue;
            }

            let currencies = graph.currencies();
            let legs: Vec<(Currency, Currency)> = cycle
                .windows(2)
                .map(|w| (currencies[w[0]].clone(), currencies[w[1]].clone()))
                .collect();
            if legs.len() != 3 {
                continue;
            }

            let triangle = Triangle {
                leg_a: legs[0].clone(),
                leg_b: legs[1].clone(),
                leg_c: legs[2].clone(),
                opportunity_bps: profit_bps,
            };

            let routes = vec![
                RouteLeg {
                    symbol: dummy_symbol(&legs[0].0, &legs[0].1),
                    side: ta_core::OrderSide::Buy,
                    price: 0,
                    size: 0,
                },
                RouteLeg {
                    symbol: dummy_symbol(&legs[1].0, &legs[1].1),
                    side: ta_core::OrderSide::Sell,
                    price: 0,
                    size: 0,
                },
                RouteLeg {
                    symbol: dummy_symbol(&legs[2].0, &legs[2].1),
                    side: ta_core::OrderSide::Sell,
                    price: 0,
                    size: 0,
                },
            ];

            opportunities.push(ArbitrageOpportunity {
                triangle,
                routes,
                expected_profit_bps: profit_bps - fee_threshold,
                ts_ns: 0,
            });
        }

        opportunities
    }
}

fn dummy_symbol(base: &str, quote: &str) -> ta_core::of_core::SymbolId {
    ta_core::of_core::SymbolId {
        venue: "BINANCE".into(),
        symbol: format!("{base}{quote}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_no_graph() {
        let engine = DetectionEngine::with_defaults();
        let graph = ExchangeRateGraph::new();
        let ops = engine.detect(&graph);
        assert!(ops.is_empty());
    }
}
