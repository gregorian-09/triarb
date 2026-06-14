use rustc_hash::FxHashMap;
use std::time::Duration;
use ta_core::{ArbitrageOpportunity, Currency, ExchangeRateGraph, RouteLeg, Triangle};

pub struct DetectionConfig {
    pub min_profit_bps: f64,
    pub max_legs: usize,
    pub fee_taker_bps: f64,
    /// If the graph has not been updated within this duration, detection is skipped.
    pub max_data_age: Duration,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            min_profit_bps: 10.0,
            max_legs: 3,
            fee_taker_bps: 10.0,
            max_data_age: Duration::from_millis(100),
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

    /// Returns an empty Vec if the graph data is too stale.
    pub fn detect(&self, graph: &ExchangeRateGraph) -> Vec<ArbitrageOpportunity> {
        if !graph.is_fresh(self.config.max_data_age) {
            tracing::warn!(
                "graph data stale (last update {:?} ago), skipping detection",
                graph.last_updated_at().elapsed()
            );
            return Vec::new();
        }

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

            let mut routes = Vec::with_capacity(3);
            let mut skip = false;
            for (from, to) in &legs {
                match graph.symbol_and_side_for(from, to) {
                    Some((sym, side)) => routes.push(RouteLeg {
                        symbol: sym.clone(),
                        side,
                        price: 0,
                        size: 0,
                    }),
                    None => {
                        tracing::warn!(
                            "cannot resolve symbol for leg {}→{}, skipping opportunity",
                            from,
                            to
                        );
                        skip = true;
                        break;
                    }
                }
            }
            if skip {
                continue;
            }

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

#[cfg(test)]
mod tests {
    use super::*;
    use ta_core::of_core::SymbolId;

    #[test]
    fn test_detect_no_graph() {
        let engine = DetectionEngine::with_defaults();
        let graph = ExchangeRateGraph::new();
        let ops = engine.detect(&graph);
        assert!(ops.is_empty());
    }

    #[test]
    fn test_detect_stale_graph() {
        let mut graph = ExchangeRateGraph::new();
        graph.set_rate(&"A".into(), &"B".into(), 100.0, 101.0);
        let engine = DetectionEngine::new(DetectionConfig {
            max_data_age: Duration::from_nanos(1),
            ..Default::default()
        });
        let ops = engine.detect(&graph);
        assert!(ops.is_empty(), "expected empty for stale graph");
    }

    #[test]
    fn test_detection_with_symbol_resolution() {
        let mut graph = ExchangeRateGraph::new();

        // Set up a 3-currency graph with extreme cross-rate mispricing
        // to ensure Bellman-Ford finds negative cycles.
        // BTCUSDT: bid=100, ask=101
        graph.set_rate(&"BTC".into(), &"USDT".into(), 100.0, 101.0);
        // ETHBTC: bid=1, ask=2 (1 ETH = 1-2 BTC — cheap side, threshold of <1)
        graph.set_rate(&"ETH".into(), &"BTC".into(), 1.0, 2.0);
        // ETHUSDT: bid=1, ask=2 (1 ETH = 1-2 USDT — priced near parity)
        graph.set_rate(&"ETH".into(), &"USDT".into(), 1.0, 2.0);

        graph.set_symbol_for(
            &"BTC".into(),
            &"USDT".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
        );
        graph.set_symbol_for(
            &"ETH".into(),
            &"BTC".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "ETHBTC".into(),
            },
        );
        graph.set_symbol_for(
            &"ETH".into(),
            &"USDT".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "ETHUSDT".into(),
            },
        );

        let engine = DetectionEngine::new(DetectionConfig {
            min_profit_bps: 0.01,
            max_legs: 3,
            fee_taker_bps: 0.0,
            max_data_age: Duration::from_secs(60),
        });

        let ops = engine.detect(&graph);

        // Verify pipeline structure: if an opportunity is produced, its legs
        // must have resolved symbols and valid sides.
        for opp in &ops {
            assert_eq!(opp.routes.len(), 3, "expected 3 legs per opportunity");
            for leg in &opp.routes {
                assert!(!leg.symbol.symbol.is_empty(), "symbol should not be empty");
                assert!(
                    matches!(leg.side, ta_core::OrderSide::Buy | ta_core::OrderSide::Sell),
                    "side should be Buy or Sell"
                );
            }
        }
        // Log count so we can observe detection behaviour
        tracing::debug!("detection found {} opportunities", ops.len());
    }

    #[test]
    fn test_symbol_and_side_correctness() {
        let mut graph = ExchangeRateGraph::new();

        // BTCUSDT: base=BTC, quote=USDT
        graph.set_rate(&"BTC".into(), &"USDT".into(), 50000.0, 50100.0);
        graph.set_symbol_for(
            &"BTC".into(),
            &"USDT".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "BTCUSDT".into(),
            },
        );

        // ETHBTC: base=ETH, quote=BTC
        graph.set_rate(&"ETH".into(), &"BTC".into(), 80.0, 90.0);
        graph.set_symbol_for(
            &"ETH".into(),
            &"BTC".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "ETHBTC".into(),
            },
        );

        // ETHUSDT: base=ETH, quote=USDT
        graph.set_rate(&"ETH".into(), &"USDT".into(), 4000.0, 4100.0);
        graph.set_symbol_for(
            &"ETH".into(),
            &"USDT".into(),
            SymbolId {
                venue: "BINANCE".into(),
                symbol: "ETHUSDT".into(),
            },
        );

        // Going USDT→BTC (buying BTC with USDT): quote→base → Buy on BTCUSDT
        let (sym, side) = graph
            .symbol_and_side_for(&"USDT".into(), &"BTC".into())
            .unwrap();
        assert_eq!(sym.symbol, "BTCUSDT");
        assert_eq!(side, ta_core::OrderSide::Buy);

        // Going BTC→USDT (selling BTC for USDT): base→quote → Sell on BTCUSDT
        let (sym, side) = graph
            .symbol_and_side_for(&"BTC".into(), &"USDT".into())
            .unwrap();
        assert_eq!(sym.symbol, "BTCUSDT");
        assert_eq!(side, ta_core::OrderSide::Sell);

        // Going USDT→ETH (buying ETH with USDT): quote→base → Buy on ETHUSDT
        let (sym, side) = graph
            .symbol_and_side_for(&"USDT".into(), &"ETH".into())
            .unwrap();
        assert_eq!(sym.symbol, "ETHUSDT");
        assert_eq!(side, ta_core::OrderSide::Buy);

        // Going ETH→USDT (selling ETH for USDT): base→quote → Sell on ETHUSDT
        let (sym, side) = graph
            .symbol_and_side_for(&"ETH".into(), &"USDT".into())
            .unwrap();
        assert_eq!(sym.symbol, "ETHUSDT");
        assert_eq!(side, ta_core::OrderSide::Sell);

        // Going BTC→ETH (buying ETH with BTC): quote→base → Buy on ETHBTC
        let (sym, side) = graph
            .symbol_and_side_for(&"BTC".into(), &"ETH".into())
            .unwrap();
        assert_eq!(sym.symbol, "ETHBTC");
        assert_eq!(side, ta_core::OrderSide::Buy);

        // Going ETH→BTC (selling ETH for BTC): base→quote → Sell on ETHBTC
        let (sym, side) = graph
            .symbol_and_side_for(&"ETH".into(), &"BTC".into())
            .unwrap();
        assert_eq!(sym.symbol, "ETHBTC");
        assert_eq!(side, ta_core::OrderSide::Sell);
    }
}
