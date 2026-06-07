pub use of_core;
use of_core::SymbolId;
use std::collections::HashMap;

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

pub enum OrderSide {
    Buy,
    Sell,
}

pub struct ExchangeRateGraph {
    n: usize,
    currencies: Vec<Currency>,
    index: HashMap<Currency, usize>,
    log_rates: Vec<f64>,
}

impl ExchangeRateGraph {
    pub fn new() -> Self {
        Self {
            n: 0,
            currencies: Vec::new(),
            index: HashMap::new(),
            log_rates: Vec::new(),
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
        idx
    }

    pub fn set_rate(&mut self, from: &Currency, to: &Currency, bid: i64, ask: i64) {
        let i = self.add_currency(from.clone());
        let j = self.add_currency(to.clone());
        if ask > 0 {
            let rate = ask as f64;
            self.log_rates[i * self.n + j] = rate.ln();
        }
        if bid > 0 {
            let rate = bid as f64;
            self.log_rates[j * self.n + i] = -(rate.recip()).ln();
        }
    }

    pub fn currencies(&self) -> &[Currency] {
        &self.currencies
    }

    pub fn detect(&self) -> Vec<(Vec<usize>, f64)> {
        if self.n == 0 {
            return Vec::new();
        }
        let mut dist = vec![f64::INFINITY; self.n];
        let mut pred = vec![0usize; self.n];
        let start = 0;
        dist[start] = 0.0;

        for _ in 0..self.n - 1 {
            for u in 0..self.n {
                for v in 0..self.n {
                    if u == v {
                        continue;
                    }
                    let w = self.log_rates[u * self.n + v];
                    if w != 0.0 && dist[u] + w < dist[v] {
                        dist[v] = dist[u] + w;
                        pred[v] = u;
                    }
                }
            }
        }

        let mut opportunities = Vec::new();
        for u in 0..self.n {
            for v in 0..self.n {
                if u == v {
                    continue;
                }
                let w = self.log_rates[u * self.n + v];
                if w != 0.0 && dist[u] + w < dist[v] - 1e-12 {
                    let mut cycle = Vec::new();
                    let mut seen = vec![false; self.n];
                    let mut cur = u;
                    while !seen[cur] {
                        seen[cur] = true;
                        cycle.push(cur);
                        cur = pred[cur];
                    }
                    cycle.push(cur);
                    cycle.reverse();
                    let profit = -(dist[u] + w - dist[v]);
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
}
