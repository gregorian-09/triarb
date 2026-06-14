use of_core::SymbolId;
use ta_core::{ArbitrageOpportunity, OrderSide};

/// Describes a hedge order needed to neutralize a partially filled leg.
#[derive(Debug, Clone)]
pub struct HedgeSpec {
    pub symbol: SymbolId,
    pub side: OrderSide,
    pub size: i64,
    pub leg_idx: usize,
}

/// Given a filled leg, return the reverse order needed to hedge it.
pub fn hedge_spec(opp: &ArbitrageOpportunity, leg_idx: usize) -> Option<HedgeSpec> {
    let leg = opp.routes.get(leg_idx)?;
    let reverse_side = match leg.side {
        OrderSide::Buy => OrderSide::Sell,
        OrderSide::Sell => OrderSide::Buy,
    };
    Some(HedgeSpec {
        symbol: leg.symbol.clone(),
        side: reverse_side,
        size: leg.size,
        leg_idx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ta_core::{RouteLeg, Triangle};

    fn dummy_opp() -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: ("USDT".into(), "BTC".into()),
                leg_b: ("BTC".into(), "ETH".into()),
                leg_c: ("ETH".into(), "USDT".into()),
                opportunity_bps: 10.0,
            },
            routes: vec![
                RouteLeg {
                    symbol: SymbolId {
                        venue: "BINANCE".into(),
                        symbol: "BTCUSDT".into(),
                    },
                    side: OrderSide::Buy,
                    price: 50000_00_000_000,
                    size: 100,
                },
                RouteLeg {
                    symbol: SymbolId {
                        venue: "BINANCE".into(),
                        symbol: "ETHBTC".into(),
                    },
                    side: OrderSide::Sell,
                    price: 5000_00_000_000,
                    size: 50,
                },
            ],
            expected_profit_bps: 5.0,
            ts_ns: 0,
        }
    }

    #[test]
    fn test_hedge_spec_for_buy_leg() {
        let opp = dummy_opp();
        let spec = hedge_spec(&opp, 0).unwrap();
        assert_eq!(spec.symbol.symbol, "BTCUSDT");
        assert!(matches!(spec.side, OrderSide::Sell));
        assert_eq!(spec.size, 100);
    }

    #[test]
    fn test_hedge_spec_for_sell_leg() {
        let opp = dummy_opp();
        let spec = hedge_spec(&opp, 1).unwrap();
        assert_eq!(spec.symbol.symbol, "ETHBTC");
        assert!(matches!(spec.side, OrderSide::Buy));
        assert_eq!(spec.size, 50);
    }

    #[test]
    fn test_hedge_spec_out_of_range() {
        let opp = dummy_opp();
        assert!(hedge_spec(&opp, 5).is_none());
    }
}
