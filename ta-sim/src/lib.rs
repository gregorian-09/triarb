use of_core::{BookLevel, BookSnapshot, SymbolId};
use std::collections::{HashMap, VecDeque};
use ta_core::{Currency, ExchangeRateGraph, OrderSide};
use ta_detect::{DetectionConfig, DetectionEngine};

/// Result of simulating a market order fill against an order book.
#[derive(Debug, Clone)]
pub struct FillResult {
    pub fills: Vec<FillEvent>,
    pub avg_price: i64,
    pub total_qty: i64,
    pub slippage_bps: f64,
    pub fee_paid: i64,
}

/// A single fill event — a partial fill at a specific price level.
#[derive(Debug, Clone)]
pub struct FillEvent {
    pub price: i64,
    pub qty: i64,
}

/// Model for simulating market order fills with configurable slippage and fees.
#[derive(Debug, Clone)]
pub struct FillModel {
    /// Simulated latency in nanoseconds between decision and execution.
    pub latency_ns: u64,
    /// Taker fee in basis points applied to each fill.
    pub taker_fee_bps: f64,
    /// Slippage behaviour.
    pub slippage: SlippageModel,
}

#[derive(Debug, Clone)]
pub enum SlippageModel {
    /// Fill at top-of-book regardless of size (optimistic).
    None,
    /// Walk the book, consuming liquidity levels until the order is filled.
    Walk,
    /// Apply a fixed additional slippage in basis points on top of top-of-book.
    Fixed(f64),
}

impl Default for FillModel {
    fn default() -> Self {
        Self {
            latency_ns: 50_000, // 50µs
            taker_fee_bps: 10.0, // 10bps
            slippage: SlippageModel::Walk,
        }
    }
}

impl FillModel {
    /// Simulate executing a market order of `size` units on the given `side`
    /// against the current book snapshot.
    ///
    /// Returns the fills, average price, total filled quantity, slippage in bps,
    /// and total fee paid.
    pub fn execute(&self, side: OrderSide, size: i64, book: &BookSnapshot) -> FillResult {
        let levels: &[BookLevel] = match side {
            OrderSide::Buy => &book.asks,
            OrderSide::Sell => &book.bids,
        };

        let top_price = levels.first().map(|l| l.price).unwrap_or(0);
        let mut remaining = size;
        let mut fills = Vec::new();

        for level in levels {
            if remaining <= 0 {
                break;
            }
            let fill_qty = remaining.min(level.size);
            fills.push(FillEvent {
                price: level.price,
                qty: fill_qty,
            });
            remaining -= fill_qty;
        }

        let total_filled = size - remaining;
        if total_filled == 0 || fills.is_empty() {
            return FillResult {
                fills: Vec::new(),
                avg_price: 0,
                total_qty: 0,
                slippage_bps: 0.0,
                fee_paid: 0,
            };
        }

        let total_notional: i64 = fills.iter().map(|f| f.price.saturating_mul(f.qty)).sum();
        let avg_price = total_notional / total_filled;

        // Slippage in bps: how far the average price is from the top-of-book
        let slippage_bps = match side {
            OrderSide::Buy => {
                if top_price > 0 {
                    ((avg_price - top_price) as f64 / top_price as f64) * 10_000.0
                } else {
                    0.0
                }
            }
            OrderSide::Sell => {
                if top_price > 0 {
                    ((top_price - avg_price) as f64 / top_price as f64) * 10_000.0
                } else {
                    0.0
                }
            }
        };

        // Apply additional fixed slippage if configured
        let adjusted_avg = match (&self.slippage, side) {
            (SlippageModel::Fixed(bps), OrderSide::Buy) => {
                avg_price + (avg_price as f64 * bps / 10_000.0) as i64
            }
            (SlippageModel::Fixed(bps), OrderSide::Sell) => {
                avg_price - (avg_price as f64 * bps / 10_000.0) as i64
            }
            _ => avg_price,
        };

        let fee_paid = (adjusted_avg as f64 * total_filled as f64 * self.taker_fee_bps / 10_000.0)
            as i64;

        FillResult {
            fills,
            avg_price: adjusted_avg,
            total_qty: total_filled,
            slippage_bps,
            fee_paid,
        }
    }
}

/// Maps a trading symbol to its base and quote currencies for graph building.
#[derive(Debug, Clone)]
pub struct SymbolMapping {
    pub symbol: String,
    pub venue: String,
    pub base: Currency,
    pub quote: Currency,
}

/// Configuration for the backtest engine.
#[derive(Debug, Clone)]
pub struct BacktestConfig {
    /// Minimum profit threshold in bps (before fees).
    pub min_profit_bps: f64,
    /// Fixed order size in the *input* currency per leg
    /// (e.g. for a Buy, this is in quote currency you spend).
    pub order_size: i64,
    /// Starting capital amount (in the same unit as prices).
    pub starting_capital: f64,
    /// Currency the starting capital is denominated in (e.g. "USDT").
    pub home_currency: Currency,
    /// Divide stored prices by this factor to get the real price
    /// in starting_capital units.  Binance USDT pairs use ~1e6.
    pub price_divisor: f64,
    /// Run detection every N ticks.
    pub detect_interval_ticks: usize,
    /// Fill model for simulating execution.
    pub fill_model: FillModel,
    /// Symbol-to-currency mappings.
    pub symbols: Vec<SymbolMapping>,
    /// Fee taker rate in bps for the detection profit filter.
    pub fee_taker_bps: f64,
    /// Maximum staledata age for the graph.
    pub max_data_age_ms: u64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            min_profit_bps: 10.0,
            order_size: 10_000,
            starting_capital: 10_000.0,
            home_currency: "USDT".into(),
            price_divisor: 1_000_000.0,
            detect_interval_ticks: 1,
            fill_model: FillModel::default(),
            symbols: Vec::new(),
            fee_taker_bps: 10.0,
            max_data_age_ms: 5000,
        }
    }
}

/// A single simulated trade produced by the backtest engine.
#[derive(Debug, Clone)]
pub struct SimulatedTrade {
    pub ts_ns: u64,
    pub profit_bps: f64,
    pub profit_quote: f64,
    pub legs: Vec<LegFill>,
}

/// Fill + currency conversion result for one leg of a simulated trade.
#[derive(Debug, Clone)]
pub struct LegFill {
    pub symbol: SymbolId,
    pub side: OrderSide,
    pub input_currency: Currency,
    pub output_currency: Currency,
    pub input_amount: f64,
    pub output_amount: f64,
    pub fill: FillResult,
}

/// Aggregated result from running a backtest.
#[derive(Debug, Clone)]
pub struct BacktestResult {
    pub trades: Vec<SimulatedTrade>,
    pub total_ticks: usize,
    pub total_opportunities_found: usize,
    pub total_executed: usize,
    pub profitable_trades: usize,
    pub unprofitable_trades: usize,
    pub total_profit_quote: f64,
    pub total_fees_paid: i64,
    pub avg_profit_bps: f64,
    pub max_profit_bps: f64,
    pub max_loss_bps: f64,
}

impl BacktestResult {
    pub fn win_rate(&self) -> f64 {
        if self.total_executed == 0 {
            return 0.0;
        }
        self.profitable_trades as f64 / self.total_executed as f64
    }

    /// Write trades as JSONL consumable by the Python ta-analysis package.
    pub fn write_jsonl(&self, path: &str) -> Result<(), std::io::Error> {
        use std::io::Write;
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        for trade in &self.trades {
            let row = BacktestJsonRow::from_trade(trade, self.total_fees_paid);
            let line = serde_json::to_string(&row).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })?;
            writeln!(writer, "{}", line)?;
        }
        writer.flush()?;
        Ok(())
    }
}

/// Flat JSON row matching Python ta_analysis.models.RawBacktestRow.
#[derive(serde::Serialize)]
pub struct BacktestJsonRow {
    pub ts_ns: u64,
    pub leg_a_symbol: String,
    pub leg_b_symbol: String,
    pub leg_c_symbol: String,
    pub profit_bps: f64,
    pub expected_profit_bps: f64,
    pub executed: bool,
    pub fill_price_a: Option<f64>,
    pub fill_price_b: Option<f64>,
    pub fill_price_c: Option<f64>,
    pub pnl_usdt: Option<f64>,
}

impl BacktestJsonRow {
    pub fn from_trade(trade: &SimulatedTrade, _total_fees: i64) -> Self {
        let legs: Vec<&str> = trade
            .legs
            .iter()
            .map(|l| l.symbol.symbol.as_str())
            .collect();
        let fill_prices: Vec<Option<f64>> = trade
            .legs
            .iter()
            .map(|l| Some(l.fill.avg_price as f64))
            .collect();

        BacktestJsonRow {
            ts_ns: trade.ts_ns,
            leg_a_symbol: legs.first().copied().unwrap_or("").to_string(),
            leg_b_symbol: legs.get(1).copied().unwrap_or("").to_string(),
            leg_c_symbol: legs.get(2).copied().unwrap_or("").to_string(),
            profit_bps: trade.profit_bps,
            expected_profit_bps: trade.profit_bps, // close approximation in simulation
            executed: true,
            fill_price_a: fill_prices.first().copied().flatten(),
            fill_price_b: fill_prices.get(1).copied().flatten(),
            fill_price_c: fill_prices.get(2).copied().flatten(),
            pnl_usdt: Some(trade.profit_quote),
        }
    }
}

/// Engine that replays historical tick data, runs triangular arbitrage detection,
/// and simulates execution with configurable fill model, slippage, and fees.
pub struct BacktestEngine {
    sim: SimulatedExchange,
    graph: ExchangeRateGraph,
    detector: DetectionEngine,
    config: BacktestConfig,
    /// Pre-built lookup: SymbolId → (base, quote)
    symbol_map: HashMap<SymbolId, (Currency, Currency)>,
}

impl BacktestEngine {
    pub fn new(config: BacktestConfig) -> Self {
        let detect_cfg = DetectionConfig {
            min_profit_bps: config.min_profit_bps,
            max_legs: 3,
            fee_taker_bps: config.fee_taker_bps,
            max_data_age: std::time::Duration::from_millis(config.max_data_age_ms),
        };
        let mut graph = ExchangeRateGraph::new();
        let mut symbol_map = HashMap::new();
        for sm in &config.symbols {
            graph.add_currency(sm.base.clone());
            graph.add_currency(sm.quote.clone());
            let sid = SymbolId {
                venue: sm.venue.clone(),
                symbol: sm.symbol.clone(),
            };
            symbol_map.insert(sid, (sm.base.clone(), sm.quote.clone()));
        }
        Self {
            sim: SimulatedExchange::new(Vec::new()),
            graph,
            detector: DetectionEngine::new(detect_cfg),
            config,
            symbol_map,
        }
    }

    /// Load historical tick data from a JSONL file.
    pub fn load_ticks(&mut self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.sim = SimulatedExchange::from_jsonl(path)?;
        Ok(())
    }

    pub fn tick_count(&self) -> usize {
        self.sim.len()
    }

    /// Run the full backtest over the loaded tick data.
    pub fn run(mut self) -> BacktestResult {
        let mut trades = Vec::new();

        while let Some(tick) = self.sim.advance() {
            let symbol = tick.symbol.clone();
            let bid = tick.bid;
            let ask = tick.ask;

            if let Some((base, quote)) = self.symbol_map.get(&symbol) {
                self.graph.set_rate(base, quote, bid, ask);
                self.graph
                    .set_symbol_for(base, quote, symbol.clone());
            }

            if self.sim.position() % self.config.detect_interval_ticks != 0 {
                continue;
            }

            let opportunities = self.detector.detect(&self.graph);
            for opp in &opportunities {
                if opp.expected_profit_bps <= 0.0 {
                    continue;
                }
                if let Some(trade) = self.simulate_trade(&opp) {
                    trades.push(trade);
                }
            }
        }

        let total_opps: usize = trades.len();
        let total_executed = trades.len();
        let profitable = trades.iter().filter(|t| t.profit_quote > 0.0).count();
        let unprofitable = trades.iter().filter(|t| t.profit_quote <= 0.0).count();
        let total_profit: f64 = trades.iter().map(|t| t.profit_quote).sum();
        let total_fees: i64 = trades
            .iter()
            .flat_map(|t| t.legs.iter().map(|l| l.fill.fee_paid))
            .sum();

        let avg_bps = if trades.is_empty() {
            0.0
        } else {
            trades.iter().map(|t| t.profit_bps).sum::<f64>() / trades.len() as f64
        };
        let max_profit = trades
            .iter()
            .map(|t| t.profit_bps)
            .fold(0_f64, f64::max);
        let max_loss = trades
            .iter()
            .map(|t| t.profit_bps)
            .fold(0_f64, f64::min);

        BacktestResult {
            trades,
            total_ticks: self.sim.position(),
            total_opportunities_found: total_opps,
            total_executed,
            profitable_trades: profitable,
            unprofitable_trades: unprofitable,
            total_profit_quote: total_profit,
            total_fees_paid: total_fees,
            avg_profit_bps: avg_bps,
            max_profit_bps: max_profit,
            max_loss_bps: max_loss,
        }
    }

    fn currencies_for(&self, symbol: &SymbolId) -> Option<&(Currency, Currency)> {
        self.symbol_map.get(symbol)
    }

    /// Rotate the routes so the first leg's input currency matches home_currency.
    fn normalize_routes(&self, opp: &ta_core::ArbitrageOpportunity) -> Option<Vec<ta_core::RouteLeg>> {
        let home = &self.config.home_currency;
        let n = opp.routes.len();
        if n == 0 {
            return None;
        }

        // Find a route whose input currency matches home
        let start_idx = opp.routes.iter().position(|leg| {
            self.currencies_for(&leg.symbol)
                .map(|(base, quote)| match leg.side {
                    OrderSide::Buy => quote == home,
                    OrderSide::Sell => base == home,
                })
                .unwrap_or(false)
        })?;

        // Rotate: take routes[start_idx..] + routes[..start_idx]
        let mut rotated = Vec::with_capacity(n);
        for i in start_idx..n {
            rotated.push(opp.routes[i].clone());
        }
        for i in 0..start_idx {
            rotated.push(opp.routes[i].clone());
        }
        // Verify the cycle still closes (last output == first input == home)
        if let Some(last) = rotated.last() {
            let last_output = self.currencies_for(&last.symbol).map(|(base, quote)| match last.side {
                OrderSide::Buy => base.clone(),
                OrderSide::Sell => quote.clone(),
            });
            if last_output.as_ref() != Some(home) {
                return None; // rotation broke the cycle
            }
        }
        Some(rotated)
    }

    fn simulate_trade(&self, opp: &ta_core::ArbitrageOpportunity) -> Option<SimulatedTrade> {
        let routes = self.normalize_routes(opp)?;
        let divisor = self.config.price_divisor;
        let order_size = self.config.order_size;
        let fee = self.config.fill_model.taker_fee_bps / 10_000.0;
        let mut legs = Vec::new();

        // Compute the effective return through all 3 legs.
        // Each leg's output/input conversion ratio gives the multiplicative return.
        // For a cycle starting and ending in home_currency:
        //   total_return = leg1_ratio * leg2_ratio * leg3_ratio * (1-fee)^3 - 1
        let mut cumulative_return = 1.0_f64;

        for leg in &routes {
            let book = self.sim.book(&leg.symbol)?;
            let (base, quote) = self.currencies_for(&leg.symbol)?.clone();
            let (input_cur, output_cur) = match leg.side {
                OrderSide::Buy => (quote.clone(), base.clone()),
                OrderSide::Sell => (base.clone(), quote.clone()),
            };

            let fill = self.config.fill_model.execute(leg.side, order_size, book);
            if fill.total_qty == 0 {
                return None;
            }

            // Conversion ratio for this leg:
            //   Buy:  we spend quote, receive base.  ratio = total_qty / (total_qty * norm_avg) = 1/norm_avg
            //         better: we started with `size * norm_avg` quote and now have `size` base.
            //         effective rate = base / quote = 1 / norm_avg
            //   Sell: we spend base, receive quote. ratio = (total_qty * norm_avg) / total_qty = norm_avg
            let norm_avg = fill.avg_price as f64 / divisor;
            let leg_return = match leg.side {
                OrderSide::Buy => 1.0 / norm_avg,
                OrderSide::Sell => norm_avg,
            } * (1.0 - fee);

            cumulative_return *= leg_return;

            legs.push(LegFill {
                symbol: leg.symbol.clone(),
                side: leg.side,
                input_currency: input_cur,
                output_currency: output_cur,
                input_amount: 0.0,
                output_amount: 0.0,
                fill,
            });
        }

        let profit_quote = self.config.starting_capital * (cumulative_return - 1.0);
        let profit_bps = (cumulative_return - 1.0) * 10_000.0;

        Some(SimulatedTrade {
            ts_ns: opp.ts_ns,
            profit_bps,
            profit_quote,
            legs,
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RawTick {
    pub ts_ns: u64,
    pub venue: String,
    pub symbol: String,
    pub bid: i64,
    pub ask: i64,
    pub last_price: i64,
    pub last_size: i64,
}

pub struct Tick {
    pub ts_ns: u64,
    pub symbol: SymbolId,
    pub bid: i64,
    pub ask: i64,
    pub last_price: i64,
    pub last_size: i64,
}

pub struct SimulatedExchange {
    ticks: VecDeque<Tick>,
    current_idx: usize,
    books: std::collections::HashMap<SymbolId, BookSnapshot>,
}

impl RawTick {
    pub fn into_tick(self) -> Tick {
        Tick {
            ts_ns: self.ts_ns,
            symbol: SymbolId {
                venue: self.venue,
                symbol: self.symbol,
            },
            bid: self.bid,
            ask: self.ask,
            last_price: self.last_price,
            last_size: self.last_size,
        }
    }
}

impl SimulatedExchange {
    pub fn new(ticks: Vec<Tick>) -> Self {
        Self {
            ticks: VecDeque::from(ticks),
            current_idx: 0,
            books: std::collections::HashMap::new(),
        }
    }

    pub fn from_jsonl(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut ticks = VecDeque::new();

        for line in std::io::BufRead::lines(reader) {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let raw: RawTick = serde_json::from_str(&line)?;
            ticks.push_back(raw.into_tick());
        }

        Ok(Self {
            ticks,
            current_idx: 0,
            books: std::collections::HashMap::new(),
        })
    }

    pub fn advance(&mut self) -> Option<&Tick> {
        let tick = self.ticks.get(self.current_idx)?;
        self.current_idx += 1;

        let snap = self
            .books
            .entry(tick.symbol.clone())
            .or_insert_with(|| BookSnapshot {
                symbol: tick.symbol.clone(),
                bids: vec![BookLevel {
                    price: tick.bid,
                    size: i64::MAX / 2,
                    level: 0,
                }],
                asks: vec![BookLevel {
                    price: tick.ask,
                    size: i64::MAX / 2,
                    level: 0,
                }],
                last_sequence: self.current_idx as u64,
                ts_exchange_ns: tick.ts_ns,
                ts_recv_ns: tick.ts_ns,
            });

        if let Some(bid) = snap.bids.first_mut() {
            bid.price = tick.bid;
        }
        if let Some(ask) = snap.asks.first_mut() {
            ask.price = tick.ask;
        }
        snap.last_sequence = self.current_idx as u64;
        snap.ts_exchange_ns = tick.ts_ns;
        snap.ts_recv_ns = tick.ts_ns;

        Some(tick)
    }

    pub fn reset(&mut self) {
        self.current_idx = 0;
        self.books.clear();
    }

    pub fn len(&self) -> usize {
        self.ticks.len()
    }

    pub fn position(&self) -> usize {
        self.current_idx
    }

    pub fn book(&self, symbol: &SymbolId) -> Option<&BookSnapshot> {
        self.books.get(symbol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use of_core::BookLevel;

    fn sample_book() -> BookSnapshot {
        BookSnapshot {
            symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() },
            bids: vec![
                BookLevel { price: 100_000, size: 10_000, level: 0 },
                BookLevel { price: 99_900, size: 20_000, level: 1 },
                BookLevel { price: 99_800, size: 30_000, level: 2 },
            ],
            asks: vec![
                BookLevel { price: 100_100, size: 10_000, level: 0 },
                BookLevel { price: 100_200, size: 20_000, level: 1 },
                BookLevel { price: 100_300, size: 30_000, level: 2 },
            ],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        }
    }

    #[test]
    fn test_fill_no_slippage_small_order() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Buy, 1_000, &book);
        assert_eq!(result.total_qty, 1_000);
        assert_eq!(result.avg_price, 100_100); // top of ask
        assert!(result.slippage_bps < 0.1);
    }

    #[test]
    fn test_fill_sell_no_slippage() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Sell, 5_000, &book);
        assert_eq!(result.total_qty, 5_000);
        assert_eq!(result.avg_price, 100_000); // top of bid
    }

    #[test]
    fn test_fill_walks_book() {
        let model = FillModel {
            slippage: SlippageModel::Walk,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        // 25_000 units: consumes 10k at level 0 + 15k at level 1
        let result = model.execute(OrderSide::Buy, 25_000, &book);
        assert_eq!(result.total_qty, 25_000);
        assert_eq!(result.fills.len(), 2);
        // Weighted average: (10k * 100100 + 15k * 100200) / 25k
        let expected_avg = (10_000i64 * 100_100 + 15_000 * 100_200) / 25_000;
        assert_eq!(result.avg_price, expected_avg);
        assert!(result.slippage_bps > 0.0);
    }

    #[test]
    fn test_fill_partial_exhausts_book() {
        let model = FillModel {
            slippage: SlippageModel::Walk,
            taker_fee_bps: 0.0,
            ..Default::default()
        };
        let book = sample_book();
        // 100_000 units: book only has 60k on ask side
        let result = model.execute(OrderSide::Buy, 100_000, &book);
        assert_eq!(result.total_qty, 60_000); // only what's available
        assert_eq!(result.fills.len(), 3);
    }

    #[test]
    fn test_fill_fees_applied() {
        let model = FillModel {
            slippage: SlippageModel::None,
            taker_fee_bps: 10.0, // 10bps
            ..Default::default()
        };
        let book = sample_book();
        let result = model.execute(OrderSide::Buy, 10_000, &book);
        assert_eq!(result.total_qty, 10_000);
        // 10bps of 100_100 * 10_000 = 10.01 * 10_000 = 100_100
        assert!(result.fee_paid > 0);
    }

    #[test]
    fn test_fill_empty_book() {
        let model = FillModel::default();
        let book = BookSnapshot {
            symbol: SymbolId { venue: "BINANCE".into(), symbol: "VOID".into() },
            bids: vec![],
            asks: vec![],
            last_sequence: 0,
            ts_exchange_ns: 0,
            ts_recv_ns: 0,
        };
        let result = model.execute(OrderSide::Buy, 100, &book);
        assert_eq!(result.total_qty, 0);
        assert!(result.fills.is_empty());
    }

    #[test]
    fn test_tick_advance() {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "BTCUSDT".into(),
        };

        let ticks = vec![
            Tick {
                ts_ns: 1,
                symbol: symbol.clone(),
                bid: 50000_00_000_000,
                ask: 50001_00_000_000,
                last_price: 50000_00_000_000,
                last_size: 100,
            },
            Tick {
                ts_ns: 2,
                symbol: symbol.clone(),
                bid: 50001_00_000_000,
                ask: 50002_00_000_000,
                last_price: 50001_00_000_000,
                last_size: 200,
            },
        ];

        let mut sim = SimulatedExchange::new(ticks);
        assert_eq!(sim.len(), 2);

        let t1 = sim.advance().unwrap();
        assert_eq!(t1.ts_ns, 1);

        let t2 = sim.advance().unwrap();
        assert_eq!(t2.ts_ns, 2);

        assert!(sim.advance().is_none());
    }

    #[test]
    fn test_backtest_synthetic_triangle() {
        let symbols = vec![
            SymbolMapping {
                symbol: "BTCUSDT".into(),
                venue: "BINANCE".into(),
                base: "BTC".into(),
                quote: "USDT".into(),
            },
            SymbolMapping {
                symbol: "ETHBTC".into(),
                venue: "BINANCE".into(),
                base: "ETH".into(),
                quote: "BTC".into(),
            },
            SymbolMapping {
                symbol: "ETHUSDT".into(),
                venue: "BINANCE".into(),
                base: "ETH".into(),
                quote: "USDT".into(),
            },
        ];

        // Extreme mispricing: ETHUSDT priced near BTCUSDT instead of ~5000
        // BTCUSDT bid=100, ask=101 → ETH should be ~5000 USDT
        // ETHUSDT bid=100, ask=101 → massive underpricing → arbitrage!
        let config = BacktestConfig {
            min_profit_bps: 0.01,
            order_size: 100,
            starting_capital: 10_000.0,
            detect_interval_ticks: 1,
            fill_model: FillModel {
                slippage: SlippageModel::None,
                taker_fee_bps: 0.0,
                ..Default::default()
            },
            fee_taker_bps: 0.0,
            max_data_age_ms: 60_000,
            home_currency: "USDT".into(),
            price_divisor: 1.0,
            symbols,
        };

        let mut engine = BacktestEngine::new(config);

        let ticks = vec![
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() },
                bid: 100,
                ask: 101,
                last_price: 100,
                last_size: 100,
            },
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() },
                bid: 50,
                ask: 51,
                last_price: 50,
                last_size: 100,
            },
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() },
                bid: 100,
                ask: 101,
                last_price: 100,
                last_size: 100,
            },
        ];

        engine.sim = SimulatedExchange::new(ticks);
        let result = engine.run();

        assert!(
            result.total_opportunities_found > 0,
            "expected at least 1 arb with extreme mispricing, got {}",
            result.total_opportunities_found
        );
        assert_eq!(result.total_ticks, 3);
    }

    #[test]
    fn test_backtest_jsonl_output() {
        let mut engine = BacktestEngine::new(BacktestConfig {
            min_profit_bps: 0.01,
            order_size: 100,
            starting_capital: 10_000.0,
            detect_interval_ticks: 1,
            fill_model: FillModel {
                slippage: SlippageModel::None,
                taker_fee_bps: 0.0,
                ..Default::default()
            },
            fee_taker_bps: 0.0,
            max_data_age_ms: 60_000,
            home_currency: "USDT".into(),
            price_divisor: 1.0,
            symbols: vec![
                SymbolMapping {
                    symbol: "BTCUSDT".into(),
                    venue: "BINANCE".into(),
                    base: "BTC".into(),
                    quote: "USDT".into(),
                },
                SymbolMapping {
                    symbol: "ETHBTC".into(),
                    venue: "BINANCE".into(),
                    base: "ETH".into(),
                    quote: "BTC".into(),
                },
                SymbolMapping {
                    symbol: "ETHUSDT".into(),
                    venue: "BINANCE".into(),
                    base: "ETH".into(),
                    quote: "USDT".into(),
                },
            ],
        });

        let ticks = vec![
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() },
                bid: 100, ask: 101, last_price: 100, last_size: 100,
            },
            Tick {
                ts_ns: 2,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() },
                bid: 50, ask: 51, last_price: 50, last_size: 100,
            },
            Tick {
                ts_ns: 3,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() },
                bid: 100, ask: 101, last_price: 100, last_size: 100,
            },
        ];

        engine.sim = SimulatedExchange::new(ticks);
        let result = engine.run();

        let path = "/tmp/test_backtest.jsonl";
        result.write_jsonl(path).unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(!content.is_empty(), "JSONL output should not be empty");
        let first_line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(first_line["executed"], true);
        assert!(first_line["profit_bps"].as_f64().unwrap() > 0.0);
        // With route normalization to start at home_currency (USDT):
        // original cycle ETH→BTC→USDT→ETH becomes USDT→ETH→BTC→USDT
        // routes: Buy(ETHUSDT), Sell(ETHBTC), Sell(BTCUSDT)
        assert_eq!(first_line["leg_a_symbol"], "ETHUSDT");
        assert_eq!(first_line["leg_b_symbol"], "ETHBTC");
        assert_eq!(first_line["leg_c_symbol"], "BTCUSDT");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_backtest_no_false_positive_consistent_rates() {
        let symbols = vec![
            SymbolMapping {
                symbol: "BTCUSDT".into(),
                venue: "BINANCE".into(),
                base: "BTC".into(),
                quote: "USDT".into(),
            },
            SymbolMapping {
                symbol: "ETHBTC".into(),
                venue: "BINANCE".into(),
                base: "ETH".into(),
                quote: "BTC".into(),
            },
            SymbolMapping {
                symbol: "ETHUSDT".into(),
                venue: "BINANCE".into(),
                base: "ETH".into(),
                quote: "USDT".into(),
            },
        ];

        // Consistent cross-rates: 100 * 50 = 5000 = bid(ETHUSDT), no arb
        let config = BacktestConfig {
            min_profit_bps: 10.0,
            order_size: 100,
            starting_capital: 10_000.0,
            detect_interval_ticks: 1,
            fill_model: FillModel {
                slippage: SlippageModel::None,
                taker_fee_bps: 10.0,
                ..Default::default()
            },
            fee_taker_bps: 10.0,
            max_data_age_ms: 60_000,
            home_currency: "USDT".into(),
            price_divisor: 1.0,
            symbols,
        };

        let mut engine = BacktestEngine::new(config);

        let ticks = vec![
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "BTCUSDT".into() },
                bid: 100,
                ask: 101,
                last_price: 100,
                last_size: 100,
            },
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHBTC".into() },
                bid: 50,
                ask: 51,
                last_price: 50,
                last_size: 100,
            },
            Tick {
                ts_ns: 1,
                symbol: SymbolId { venue: "BINANCE".into(), symbol: "ETHUSDT".into() },
                bid: 4999,
                ask: 5000,
                last_price: 4999,
                last_size: 100,
            },
        ];

        engine.sim = SimulatedExchange::new(ticks);
        let result = engine.run();

        assert!(
            result.total_opportunities_found < 2,
            "expected no arb with consistent rates, got {}",
            result.total_opportunities_found
        );
    }

    #[test]
    fn test_reset() {
        let symbol = SymbolId {
            venue: "BINANCE".into(),
            symbol: "ETHUSDT".into(),
        };

        let mut sim = SimulatedExchange::new(vec![Tick {
            ts_ns: 1,
            symbol,
            bid: 3000_00_000_000,
            ask: 3001_00_000_000,
            last_price: 3000_00_000_000,
            last_size: 10,
        }]);

        assert_eq!(sim.position(), 0);
        sim.advance();
        assert_eq!(sim.position(), 1);
        sim.reset();
        assert_eq!(sim.position(), 0);
    }
}
