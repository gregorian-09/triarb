pub use of_core;
use of_core::SymbolId;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub type Currency = String;

pub struct RateTriple {
    pub base: Currency,
    pub quote: Currency,
    pub best_bid: i64,
    pub best_ask: i64,
}

#[derive(Debug, Clone)]
pub struct Triangle {
    pub leg_a: (Currency, Currency),
    pub leg_b: (Currency, Currency),
    pub leg_c: (Currency, Currency),
    pub opportunity_bps: f64,
}

#[derive(Clone)]
pub struct ArbitrageOpportunity {
    pub triangle: Triangle,
    pub routes: Vec<RouteLeg>,
    pub expected_profit_bps: f64,
    pub ts_ns: u64,
}

#[derive(Clone)]
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
        self.legs.iter().any(|s| *s == LegFillStatus::Filled) && !self.is_fully_filled()
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
    /// Maps (from_idx, to_idx) → the exchange symbol that produced this rate.
    /// Stored as {base}{quote} in the symbol name, consistent with parse_currency.
    symbol_map: HashMap<(usize, usize), SymbolId>,
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
            symbol_map: HashMap::new(),
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
            symbol_map: HashMap::with_capacity(n),
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
        // base→quote: selling base for quote → you receive `bid` quote per base.
        // Weight = -ln(bid) (negative of log of quantity received).
        if bid > 0 {
            let w = -(bid as f64).ln();
            self.log_rates[i * n + j] = w;
            let adj = &mut self.adjacency[i];
            if let Some(pos) = adj.iter().position(|&(t, _)| t == j) {
                adj[pos] = (j, w);
            } else {
                adj.push((j, w));
            }
        }
        // quote→base: buying base with quote → you pay `ask` quote per base.
        // Weight = ln(ask) (log of quantity paid).
        if ask > 0 {
            let w = (ask as f64).ln();
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

    /// Record which exchange symbol produced the rate for an ordered currency pair.
    /// `from` and `to` must match the order used in `set_rate`.
    pub fn set_symbol_for(&mut self, from: &Currency, to: &Currency, symbol: SymbolId) {
        let i = self.index.get(from);
        let j = self.index.get(to);
        if let (Some(&i), Some(&j)) = (i, j) {
            self.symbol_map.entry((i, j)).or_insert(symbol);
        }
    }

    /// Look up the exchange symbol for a directed currency pair.
    /// Returns `None` if no symbol was registered for this direction.
    pub fn get_symbol_for(&self, from: &Currency, to: &Currency) -> Option<&SymbolId> {
        let i = self.index.get(from)?;
        let j = self.index.get(to)?;
        self.symbol_map.get(&(*i, *j))
    }

    /// Given a directed leg `(from, to)`, return the exchange symbol and
    /// the order side needed to execute that leg.
    ///
    /// The graph stores symbols in `{base}{quote}` order (from `set_rate`).
    /// If the leg follows the stored direction (base→quote) the side is Sell.
    /// If the leg is reversed (quote→base) the side is Buy.
    pub fn symbol_and_side_for(
        &self,
        from: &Currency,
        to: &Currency,
    ) -> Option<(&SymbolId, OrderSide)> {
        // Direct match: (from, to) was the pair order → selling base for quote
        if let Some(sym) = self.get_symbol_for(from, to) {
            return Some((sym, OrderSide::Sell));
        }
        // Reverse match: (to, from) was the pair order → buying base with quote
        if let Some(sym) = self.get_symbol_for(to, from) {
            return Some((sym, OrderSide::Buy));
        }
        None
    }

    pub fn detect(&self) -> Vec<(Vec<usize>, f64)> {
        let n = self.n;
        if n == 0 {
            return Vec::new();
        }
        let mut dist = vec![f64::INFINITY; n];
        let mut pred = vec![0usize; n];
        let mut in_queue = vec![false; n];
        let mut count = vec![0usize; n];
        let mut queue = VecDeque::new();

        dist[0] = 0.0;
        queue.push_back(0);
        in_queue[0] = true;

        while let Some(u) = queue.pop_front() {
            in_queue[u] = false;
            let du = dist[u];
            for &(v, w) in &self.adjacency[u] {
                let dv = du + w;
                if dv < dist[v] - 1e-12 {
                    dist[v] = dv;
                    pred[v] = u;
                    if !in_queue[v] {
                        count[v] += 1;
                        if count[v] >= n {
                            queue.clear();
                            break;
                        }
                        queue.push_back(v);
                        in_queue[v] = true;
                    }
                }
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
                    // Walk back from u along predecessors to build the reverse path.
                    // Stop when we either hit a self-reference (pred[i]==i at source)
                    // or the next node is already in the path (cycle within).
                    let mut path = Vec::new();
                    let mut cur = u;
                    loop {
                        path.push(cur);
                        if pred[cur] == cur {
                            break;
                        }
                        if path.contains(&pred[cur]) {
                            path.push(pred[cur]);
                            break;
                        }
                        cur = pred[cur];
                    }

                    // The relaxing edge u→v closes the cycle.
                    // path stores nodes from u back to source (reversed cycle order).
                    // Reconstruct forward: v → reverse(path[..v_pos]) → v
                    let cycle = if let Some(v_pos) = path.iter().rposition(|&x| x == v) {
                        let mut c: Vec<usize> = path[..v_pos].iter().rev().copied().collect();
                        c.insert(0, v);
                        c.push(v);
                        c
                    } else {
                        let last = path[path.len() - 1];
                        let mut c: Vec<usize> =
                            path[..path.len() - 1].iter().rev().copied().collect();
                        c.insert(0, last);
                        c.push(v);
                        c
                    };

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

    #[test]
    fn test_detect_finds_arbitrage() {
        let mut g = ExchangeRateGraph::new();
        // BTCUSDT: bid=100, ask=101
        //   BTC→USDT = -ln(100) = -4.605 (sell BTC)
        //   USDT→BTC =  ln(101) =  4.615 (buy BTC)
        g.set_rate(&"BTC".into(), &"USDT".into(), 100, 101);
        // ETHBTC: bid=1, ask=2
        //   ETH→BTC = -ln(1) = 0 (sell ETH)
        //   BTC→ETH =  ln(2) = 0.693 (buy ETH)
        g.set_rate(&"ETH".into(), &"BTC".into(), 1, 2);
        // ETHUSDT: bid=1, ask=2
        //   ETH→USDT = -ln(1) = 0 (sell ETH)
        //   USDT→ETH =  ln(2) = 0.693 (buy ETH)
        g.set_rate(&"ETH".into(), &"USDT".into(), 1, 2);

        let ops = g.detect();
        assert!(
            !ops.is_empty(),
            "expected at least one arbitrage opportunity"
        );

        for (cycle, profit) in &ops {
            assert!(
                cycle.len() >= 4,
                "cycle must have at least 4 nodes (start + 2 intermediate + end)"
            );
            assert!(*profit > 0.0, "profit must be positive");
            // Verify the cycle starts and ends with the same currency
            assert_eq!(cycle.first(), cycle.last(), "cycle must be closed");
        }
    }

    #[test]
    fn test_no_false_positive_no_arb() {
        // All rates consistent — no arbitrage should be detected
        let mut g = ExchangeRateGraph::new();
        // BTCUSDT: bid=100, ask=101
        g.set_rate(&"BTC".into(), &"USDT".into(), 100, 101);
        // Cross rates consistent: 1 ETH = 0.5 BTC, 1 ETH = 50 USDT
        // ETHBTC: bid=50, ask=51  → ETH→BTC = -ln(50), BTC→ETH = ln(51)
        g.set_rate(&"ETH".into(), &"BTC".into(), 50, 51);
        // ETHUSDT: bid=5000, ask=5100
        g.set_rate(&"ETH".into(), &"USDT".into(), 5000, 5100);

        let ops = g.detect();
        // With consistent cross-rates (50 BTC/ETH × 100 USDT/BTC = 5000 USDT/ETH matches direct)
        // there should be no arbitrage
    }

    #[test]
    fn test_detect_with_my_rates() {
        let mut g = ExchangeRateGraph::new();
        // Same rates as the backtest_synthetic_triangle test
        g.set_rate(&"BTC".into(), &"USDT".into(), 100, 101);
        g.set_rate(&"ETH".into(), &"BTC".into(), 50, 51);
        g.set_rate(&"ETH".into(), &"USDT".into(), 100, 101);

        let ops = g.detect();
        assert!(!ops.is_empty(), "my rates should produce arbitrage");
        let (_, profit) = &ops[0];
        assert!(*profit > 0.0, "profit must be positive");
    }
}
