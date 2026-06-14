use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use ta_sim::{BacktestConfig, FillModel, SlippageModel, SymbolMapping};

use crate::cli::BacktestArgs;

#[derive(Debug, Clone, Deserialize)]
pub struct BacktestToml {
    /// Per-symbol mapping: table key = symbol name
    pub symbols: HashMap<String, SymbolToml>,
    #[serde(default)]
    pub detect: DetectToml,
    #[serde(default)]
    pub execution: ExecutionToml,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolToml {
    pub venue: String,
    pub base: String,
    pub quote: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectToml {
    pub min_profit_bps: f64,
    pub fee_taker_bps: f64,
    pub max_data_age_ms: u64,
}

impl Default for DetectToml {
    fn default() -> Self {
        Self {
            min_profit_bps: 10.0,
            fee_taker_bps: 10.0,
            max_data_age_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionToml {
    pub order_size: i64,
    pub starting_capital: f64,
    pub taker_fee_bps: f64,
    pub slippage: String,
    pub detect_interval_ticks: usize,
}

impl Default for ExecutionToml {
    fn default() -> Self {
        Self {
            order_size: 10_000,
            starting_capital: 10_000.0,
            taker_fee_bps: 10.0,
            slippage: "walk".into(),
            detect_interval_ticks: 1,
        }
    }
}

pub fn run_backtest(args: &BacktestArgs) -> Result<()> {
    let toml_content = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let cfg: BacktestToml = toml::from_str(&toml_content)
        .with_context(|| format!("parsing {}", args.config.display()))?;

    let data_path = match &args.data {
        Some(p) => p.clone(),
        None => bail!("--data path is required"),
    };

    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("backtest_trades.jsonl"));

    // Build symbol mappings
    let mut symbols = Vec::new();
    for (name, sm) in &cfg.symbols {
        symbols.push(SymbolMapping {
            symbol: name.clone(),
            venue: sm.venue.clone(),
            base: sm.base.clone(),
            quote: sm.quote.clone(),
        });
    }
    if symbols.is_empty() {
        bail!("at least one symbol required in [symbols]");
    }

    // Build fill model
    let slippage = match cfg.execution.slippage.as_str() {
        "none" => SlippageModel::None,
        "walk" => SlippageModel::Walk,
        s if s.starts_with("fixed=") => {
            let bps: f64 = s[6..]
                .parse()
                .context("invalid slippage format, expected fixed=<bps>")?;
            SlippageModel::Fixed(bps)
        }
        other => bail!(
            "unknown slippage model '{other}', expected 'none', 'walk', or 'fixed=<bps>'"
        ),
    };

    let backtest_cfg = BacktestConfig {
        min_profit_bps: cfg.detect.min_profit_bps,
        order_size: cfg.execution.order_size,
        starting_capital: cfg.execution.starting_capital,
        detect_interval_ticks: cfg.execution.detect_interval_ticks,
        fill_model: FillModel {
            latency_ns: 50_000,
            taker_fee_bps: cfg.execution.taker_fee_bps,
            slippage,
        },
        symbols,
        fee_taker_bps: cfg.detect.fee_taker_bps,
        max_data_age_ms: cfg.detect.max_data_age_ms,
    };

    let mut engine = ta_sim::BacktestEngine::new(backtest_cfg);
    engine
        .load_ticks(&data_path.to_string_lossy())
        .map_err(|e| anyhow::anyhow!("loading data from {}: {}", data_path.display(), e))?;

    eprintln!(
        "running backtest on {} ticks from {}",
        engine.tick_count(),
        data_path.display()
    );

    let result = engine.run();

    result
        .write_jsonl(&output_path.to_string_lossy())
        .with_context(|| format!("writing output to {}", output_path.display()))?;

    eprintln!(
        "backtest complete: {} ticks, {} trades, {}/{} win/loss, {:.1}% win rate, {:.2} avg bps, {:.2} total pnl",
        result.total_ticks,
        result.total_executed,
        result.profitable_trades,
        result.unprofitable_trades,
        result.win_rate() * 100.0,
        result.avg_profit_bps,
        result.total_profit_quote,
    );
    eprintln!("output written to {}", output_path.display());

    Ok(())
}
