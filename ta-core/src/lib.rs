pub use of_core;
use of_core::SymbolId;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub type Currency = String;

pub struct RateTriple {
    pub base: Currency,
    pub quote: Currency,
    pub best_bid: i64,
    pub best_ask: i64,
}

pub struct Triangle {
    pub leg_a: (Currency, Currency),
    pub leg_b: (Currency, Currency),
    pub leg_c: (Currency, Currency),
    pub opportunity_bps: f64,
}

pub struct ArbitrageOpportunity {
    pub triangle: Triangle,
    pub routes: Vec<RouteLeg>,
    pub expected_profit_bps: f64,
    pub ts_ns: u64,
}

pub struct RouteLeg {
    pub symbol: SymbolId,
    pub side: OrderSide,
    pub price: i64,
    pub size: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Deterministic unique identifier for an arbitrage opportunity.
///
/// Derived from SHA-256 of the canonical triangle key so the same
/// triangle at the same timestamp window always produces the same ID.
#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub struct OpportunityId([u8; 32]);

impl std::fmt::Debug for OpportunityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0[..4] {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "..")?;
        for byte in &self.0[28..] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl OpportunityId {
    pub fn from_opportunity(opp: &ArbitrageOpportunity) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(opp.triangle.leg_a.0.as_bytes());
        hasher.update(opp.triangle.leg_a.1.as_bytes());
        hasher.update(opp.triangle.leg_b.0.as_bytes());
        hasher.update(opp.triangle.leg_b.1.as_bytes());
        hasher.update(opp.triangle.leg_c.0.as_bytes());
        hasher.update(opp.triangle.leg_c.1.as_bytes());
        let hash = hasher.finalize();
        Self(hash.into())
    }
}

/// TTL-backed deduplication table for opportunity IDs.
pub struct DedupTable {
    entries: HashMap<OpportunityId, Instant>,
    ttl: Duration,
}

impl DedupTable {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
        }
    }

    /// Returns `true` if the ID is new (not a duplicate) and inserts it.
    pub fn check_and_insert(&mut self, id: OpportunityId) -> bool {
        if self.entries.contains_key(&id) {
            return false;
        }
        self.entries.insert(id, Instant::now());
        true
    }

    /// Evict expired entries. Should be called periodically.
    pub fn gc(&mut self) {
        let cutoff = Instant::now() - self.ttl;
        self.entries.retain(|_, inserted_at| *inserted_at > cutoff);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Fill status of a single leg in an arbitrage opportunity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LegFillStatus {
    Pending,
    Filled,
    Failed,
}

/// Tracks the execution state of an arbitrage opportunity across its legs.
#[derive(Clone, Debug)]
pub struct FillState {
    pub opportunity_id: OpportunityId,
    pub legs: [LegFillStatus; 3],
    pub started_at: Instant,
}

impl FillState {
    pub fn new(opportunity_id: OpportunityId) -> Self {
        Self {
            opportunity_id,
            legs: [LegFillStatus::Pending; 3],
            started_at: Instant::now(),
        }
    }

    pub fn fill_leg(&mut self, leg: usize) {
        if leg < 3 {
            self.legs[leg] = LegFillStatus::Filled;
        }
    }

    pub fn fail_leg(&mut self, leg: usize) {
        if leg < 3 {
            self.legs[leg] = LegFillStatus::Failed;
        }
    }

    pub fn is_fully_filled(&self) -> bool {
        self.legs.iter().all(|s| *s == LegFillStatus::Filled)
    }

    pub fn has_partial_fill(&self) -> bool {
        self.legs.iter().any(|s| *s == LegFillStatus::Filled)
            && !self.is_fully_filled()
    }

    pub fn has_failure(&self) -> bool {
        self.legs.iter().any(|s| *s == LegFillStatus::Failed)
    }

    /// Returns true if this opportunity needs a rollback:
    /// at least one leg filled but not all three.
    pub fn needs_rollback(&self) -> bool {
        self.has_partial_fill() && !self.is_fully_filled()
    }
}

pub struct ExchangeRateGraph {
    n: usize,
    currencies: Vec<Currency>,
    index: HashMap<Currency, usize>,
    log_rates: Vec<f64>,
    adjacency: Vec<Vec<(usize, f64)>>,
    last_updated_at: Instant,
}

impl ExchangeRateGraph {
    pub fn new() -> Self {
        Self {
            n: 0,
            currencies: Vec::new(),
            index: HashMap::new(),
            log_rates: Vec::new(),
            adjacency: Vec::new(),
            last_updated_at: Instant::now(),
        }
    }

    pub fn with_capacity(n: usize) -> Self {
        Self {
            n: 0,
            currencies: Vec::with_capacity(n),
            index: HashMap::with_capacity(n),
            log_rates: Vec::with_capacity(n * n),
            adjacency: Vec::with_capacity(n),
            last_updated_at: Instant::now(),
        }
    }

    pub fn add_currency(&mut self, currency: Currency) -> usize {
        if let Some(&idx) = self.index.get(&currency) {
            return idx;
        }
        let idx = self.n;
        self.currencies.push(currency.clone());
        self.index.insert(currency, idx);
        self.n += 1;
        let new_size = self.n * self.n;
        self.log_rates.resize(new_size, 0.0);
        for i in 0..self.n - 1 {
            self.log_rates[i * self.n + idx] = 0.0;
            self.log_rates[idx * self.n + i] = 0.0;
        }
        self.adjacency.push(Vec::new());
        idx
    }

    pub fn set_rate(&mut self, from: &Currency, to: &Currency, bid: i64, ask: i64) {
        let i = self.add_currency(from.clone());
        let j = self.add_currency(to.clone());
        let n = self.n;
        if ask > 0 {
            let w = (ask as f64).ln();
            self.log_rates[i * n + j] = w;
            let adj = &mut self.adjacency[i];
            if let Some(pos) = adj.iter().position(|&(t, _)| t == j) {
                adj[pos] = (j, w);
            } else {
                adj.push((j, w));
            }
        }
        if bid > 0 {
            let w = -(bid as f64).recip().ln();
            self.log_rates[j * n + i] = w;
            let adj = &mut self.adjacency[j];
            if let Some(pos) = adj.iter().position(|&(t, _)| t == i) {
                adj[pos] = (i, w);
            } else {
                adj.push((i, w));
            }
        }
        self.last_updated_at = Instant::now();
    }

    pub fn currencies(&self) -> &[Currency] {
        &self.currencies
    }

    /// Monotonic timestamp of the last successful `set_rate`.
    pub fn last_updated_at(&self) -> Instant {
        self.last_updated_at
    }

    /// Returns true if the graph has received any update within the last `max_age`.
    pub fn is_fresh(&self, max_age: Duration) -> bool {
        self.last_updated_at.elapsed() <= max_age
    }

    pub fn detect(&self) -> Vec<(Vec<usize>, f64)> {
        let n = self.n;
        if n == 0 {
            return Vec::new();
        }
        let mut dist = vec![f64::INFINITY; n];
        let mut pred = vec![0usize; n];
        dist[0] = 0.0;

        for _ in 0..n - 1 {
            let mut changed = false;
            for u in 0..n {
                let du = dist[u];
                if du == f64::INFINITY {
                    continue;
                }
                for &(v, w) in &self.adjacency[u] {
                    let dv = du + w;
                    if dv < dist[v] {
                        dist[v] = dv;
                        pred[v] = u;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut opportunities = Vec::new();
        for u in 0..n {
            let du = dist[u];
            if du == f64::INFINITY {
                continue;
            }
            for &(v, w) in &self.adjacency[u] {
                if du + w < dist[v] - 1e-12 {
                    let mut cycle = Vec::with_capacity(n);
                    let mut seen = vec![false; n];
                    let mut cur = u;
                    while !seen[cur] {
                        seen[cur] = true;
                        cycle.push(cur);
                        cur = pred[cur];
                    }
                    cycle.push(cur);
                    cycle.reverse();
                    let profit = -(du + w - dist[v]);
                    opportunities.push((cycle, profit));
                }
            }
        }
        opportunities
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_arbitrage() {
        let g = ExchangeRateGraph::new();
        assert!(g.detect().is_empty());
    }

    #[test]
    fn test_opportunity_id_deterministic() {
        let opp = ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: ("USDT".into(), "BTC".into()),
                leg_b: ("BTC".into(), "ETH".into()),
                leg_c: ("ETH".into(), "USDT".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        };
        let id1 = OpportunityId::from_opportunity(&opp);
        let id2 = OpportunityId::from_opportunity(&opp);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_dedup_table() {
        let mut table = DedupTable::new(Duration::from_secs(60));
        let opp = ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: ("A".into(), "B".into()),
                leg_b: ("B".into(), "C".into()),
                leg_c: ("C".into(), "A".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        };
        let id = OpportunityId::from_opportunity(&opp);
        assert!(table.check_and_insert(id));
        assert!(!table.check_and_insert(id));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_fill_state() {
        let opp = ArbitrageOpportunity {
            triangle: Triangle {
                leg_a: ("A".into(), "B".into()),
                leg_b: ("B".into(), "C".into()),
                leg_c: ("C".into(), "A".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        };
        let id = OpportunityId::from_opportunity(&opp);
        let mut state = FillState::new(id);
        assert!(!state.needs_rollback());
        state.fill_leg(0);
        assert!(state.needs_rollback());
        state.fill_leg(1);
        state.fill_leg(2);
        assert!(!state.needs_rollback());
        assert!(state.is_fully_filled());
    }

    #[test]
    fn test_graph_freshness() {
        let mut g = ExchangeRateGraph::new();
        assert!(!g.is_fresh(Duration::from_nanos(1))); // never updated
        g.set_rate(&"A".into(), &"B".into(), 100, 101);
        assert!(g.is_fresh(Duration::from_secs(60)));
    }
}
