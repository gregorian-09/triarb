use std::sync::Mutex;
use divan::Bencher;
use of_core::BookAction;
use of_core::BookUpdate;
use rand::Rng;
use rustc_hash::FxHashMap;
use ta_core::ExchangeRateGraph;
use ta_detect::{DetectionConfig, DetectionEngine};
use ta_exec::{ExecConfig, ExecEngine};

fn main() {
    divan::main();
}

// helpers

fn build_graph(n: usize) -> ExchangeRateGraph {
    let mut g = ExchangeRateGraph::with_capacity(n);
    let currencies: Vec<String> = (0..n).map(|i| format!("CURR{i}")).collect();
    for c in &currencies {
        g.add_currency(c.clone());
    }
    let mut rng = rand::thread_rng();
    for i in 0..n {
        for j in 0..n {
            if i != j {
                let bid = rng.gen_range(100_000..200_000);
                let ask = bid + rng.gen_range(1..100);
                g.set_rate(&currencies[i], &currencies[j], bid, ask);
            }
        }
    }
    g
}

fn build_sparse_graph(n: usize) -> ExchangeRateGraph {
    let mut g = ExchangeRateGraph::with_capacity(n);
    let currencies: Vec<String> = (0..n).map(|i| format!("CURR{i}")).collect();
    for c in &currencies {
        g.add_currency(c.clone());
    }
    let mut rng = rand::thread_rng();
    for i in 0..n {
        for j in 0..n {
            if i != j && rng.gen_bool(0.15) {
                let bid = rng.gen_range(100_000..200_000);
                let ask = bid + rng.gen_range(1..100);
                g.set_rate(&currencies[i], &currencies[j], bid, ask);
            }
        }
    }
    g
}

// ── Bellman-Ford detection latency ──────────────────────────────────

#[divan::bench]
fn graph_detect_n20(bencher: Bencher) {
    let graph = build_graph(20);
    bencher.bench(|| graph.detect());
}

#[divan::bench]
fn graph_detect_n50(bencher: Bencher) {
    let graph = build_graph(50);
    bencher.bench(|| graph.detect());
}

#[divan::bench]
fn graph_detect_n100(bencher: Bencher) {
    let graph = build_graph(100);
    bencher.bench(|| graph.detect());
}

#[divan::bench]
fn graph_detect_sparse_500(bencher: Bencher) {
    let graph = build_sparse_graph(500);
    bencher.bench(|| graph.detect());
}

// ── Graph update latency (single set_rate) ──────────────────────────

#[divan::bench]
fn graph_update(bencher: Bencher) {
    let graph = Mutex::new(build_graph(20));
    bencher.bench(|| {
        graph.lock().unwrap().set_rate(&"CURR0".into(), &"CURR1".into(), 100_000, 100_050);
    });
}

#[divan::bench]
fn graph_update_100(bencher: Bencher) {
    let graph = Mutex::new(build_graph(100));
    bencher.bench(|| {
        graph.lock().unwrap().set_rate(&"CURR0".into(), &"CURR1".into(), 100_000, 100_050);
    });
}

// ── Full detection pipeline (graph detect + fee filter + triangle builder) ──

#[divan::bench]
fn detect_pipeline_n20(bencher: Bencher) {
    let engine = DetectionEngine::new(DetectionConfig {
        min_profit_bps: 1.0,
        max_legs: 3,
        fee_taker_bps: 10.0,
        max_data_age: std::time::Duration::from_secs(60),
    });
    let graph = build_graph(20);
    bencher.bench(|| engine.detect(&graph));
}

#[divan::bench]
fn detect_pipeline_n50(bencher: Bencher) {
    let engine = DetectionEngine::new(DetectionConfig {
        min_profit_bps: 1.0,
        max_legs: 3,
        fee_taker_bps: 10.0,
        max_data_age: std::time::Duration::from_secs(60),
    });
    let graph = build_graph(50);
    bencher.bench(|| engine.detect(&graph));
}

#[divan::bench]
fn detect_pipeline_n100(bencher: Bencher) {
    let engine = DetectionEngine::new(DetectionConfig {
        min_profit_bps: 1.0,
        max_legs: 3,
        fee_taker_bps: 10.0,
        max_data_age: std::time::Duration::from_secs(60),
    });
    let graph = build_graph(100);
    bencher.bench(|| engine.detect(&graph));
}

// ── Execution engine startup ────────────────────────────────────────

#[divan::bench]
fn exec_engine_startup(bencher: Bencher) {
    bencher.bench(|| {
        let _ = ExecEngine::new(ExecConfig::default());
    });
}

// ── Feed apply_book_update latency ──────────────────────────────────

fn book(symbol: of_core::SymbolId, side: of_core::Side, level: u16, price: i64, size: i64) -> BookUpdate {
    BookUpdate {
        symbol,
        side,
        level,
        price,
        size,
        action: BookAction::Upsert,
        sequence: level as u64,
        ts_exchange_ns: 1,
        ts_recv_ns: 2,
    }
}

#[divan::bench]
fn feed_book_update_new_symbol(bencher: Bencher) {
    let books: Mutex<FxHashMap<of_core::SymbolId, of_core::BookSnapshot>> =
        Mutex::new(FxHashMap::default());
    let graph: Mutex<ExchangeRateGraph> = Mutex::new(ExchangeRateGraph::with_capacity(10));

    let symbol = of_core::SymbolId {
        venue: "BINANCE".into(),
        symbol: "BTCUSDT".into(),
    };

    bencher.bench(|| {
        ta_feed::FeedEngine::bench_apply_book_update(
            &mut *books.lock().unwrap(),
            &mut *graph.lock().unwrap(),
            book(
                symbol.clone(),
                of_core::Side::Bid,
                0,
                50000_00_000_000,
                100,
            ),
        );
    });
}

#[divan::bench]
fn feed_book_update_existing_10_levels(bencher: Bencher) {
    let books: Mutex<FxHashMap<of_core::SymbolId, of_core::BookSnapshot>> =
        Mutex::new(FxHashMap::default());
    let graph: Mutex<ExchangeRateGraph> = Mutex::new(ExchangeRateGraph::with_capacity(10));
    let symbol = of_core::SymbolId {
        venue: "BINANCE".into(),
        symbol: "BTCUSDT".into(),
    };

    for level in 0u16..10 {
        ta_feed::FeedEngine::bench_apply_book_update(
            &mut *books.lock().unwrap(),
            &mut *graph.lock().unwrap(),
            book(
                symbol.clone(),
                of_core::Side::Bid,
                level,
                50000_00_000_000 + (level as i64) * 100,
                100,
            ),
        );
        ta_feed::FeedEngine::bench_apply_book_update(
            &mut *books.lock().unwrap(),
            &mut *graph.lock().unwrap(),
            book(
                symbol.clone(),
                of_core::Side::Ask,
                level,
                50001_00_000_000 + (level as i64) * 100,
                100,
            ),
        );
    }

    bencher.bench(|| {
        ta_feed::FeedEngine::bench_apply_book_update(
            &mut *books.lock().unwrap(),
            &mut *graph.lock().unwrap(),
            book(
                symbol.clone(),
                of_core::Side::Bid,
                0,
                50002_00_000_000,
                150,
            ),
        );
    });
}
