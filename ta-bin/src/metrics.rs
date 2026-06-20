use axum::{routing::get, Router};
use prometheus::{
    register_counter, register_gauge, register_histogram_vec, Counter, Encoder, Gauge,
    HistogramVec, TextEncoder,
};
use std::net::SocketAddr;
use std::sync::OnceLock;

static METRICS: OnceLock<BotMetrics> = OnceLock::new();

pub struct BotMetrics {
    pub books_received: Counter,
    pub trades_received: Counter,
    pub polls_total: Counter,
    pub opportunities_found: Counter,
    pub opportunities_executed: Counter,
    pub executions_failed: Counter,
    pub feed_reconnects: Counter,
    pub feed_connected: Gauge,
    pub detection_duration: HistogramVec,
}

fn init_metrics() -> BotMetrics {
    let m = BotMetrics {
        books_received: register_counter!("feed_books_total", "Book updates received").unwrap(),
        trades_received: register_counter!("feed_trades_total", "Trade prints received").unwrap(),
        polls_total: register_counter!("feed_polls_total", "Poll cycles completed").unwrap(),
        opportunities_found: register_counter!(
            "detect_opportunities_total",
            "Arbitrage opportunities found"
        )
        .unwrap(),
        opportunities_executed: register_counter!(
            "exec_opportunities_total",
            "Opportunities executed"
        )
        .unwrap(),
        executions_failed: register_counter!("exec_failures_total", "Execution failures").unwrap(),
        feed_reconnects: register_counter!("feed_reconnects_total", "Feed reconnection attempts")
            .unwrap(),
        feed_connected: register_gauge!("feed_connected", "Feed connection state (1=connected)")
            .unwrap(),
        detection_duration: register_histogram_vec!(
            "detect_duration_seconds",
            "Detection latency in seconds",
            &[],
            vec![0.00001, 0.00005, 0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1]
        )
        .unwrap(),
    };
    prometheus::default_registry()
        .register(Box::new(m.detection_duration.clone()))
        .ok();
    m
}

pub fn metrics() -> &'static BotMetrics {
    METRICS.get_or_init(init_metrics)
}

async fn handle_metrics() -> String {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    let metric_families = prometheus::default_registry().gather();
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap_or_default()
}

async fn handle_health() -> &'static str {
    "ok"
}

pub async fn serve_metrics(port: u16) {
    let app = Router::new()
        .route("/metrics", get(handle_metrics))
        .route("/health", get(handle_health));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(addr = %addr, "metrics server listening");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
