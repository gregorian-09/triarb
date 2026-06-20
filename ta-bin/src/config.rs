use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub symbols: Vec<String>,
    pub endpoint: Option<String>,
    pub feed: FeedCfg,
    pub detect: DetectCfg,
    pub logging: LoggingCfg,
    pub metrics_port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedCfg {
    pub message_timeout_secs: u64,
    pub poll_interval_ms: u64,
    pub reconnect_base_ms: u64,
    pub reconnect_max_ms: u64,
}

impl Default for FeedCfg {
    fn default() -> Self {
        Self {
            message_timeout_secs: 10,
            poll_interval_ms: 50,
            reconnect_base_ms: 250,
            reconnect_max_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectCfg {
    pub min_profit_bps: f64,
    pub max_legs: usize,
    pub fee_taker_bps: f64,
    pub max_data_age_ms: u64,
}

impl Default for DetectCfg {
    fn default() -> Self {
        Self {
            min_profit_bps: 10.0,
            max_legs: 3,
            fee_taker_bps: 10.0,
            max_data_age_ms: 200,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingCfg {
    pub directory: Option<String>,
    pub level: String,
    pub format: String,
}

impl Default for LoggingCfg {
    fn default() -> Self {
        Self {
            directory: None,
            level: "info".to_string(),
            format: "json".to_string(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            symbols: vec!["BTCUSDT".into(), "ETHUSDT".into(), "ETHBTC".into()],
            endpoint: Some("wss://stream.binance.com:9443/ws".into()),
            feed: FeedCfg::default(),
            detect: DetectCfg::default(),
            logging: LoggingCfg::default(),
            metrics_port: 9100,
        }
    }
}

impl AppConfig {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&contents)?;
        Ok(cfg)
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.feed.poll_interval_ms)
    }

    pub fn message_timeout(&self) -> Duration {
        Duration::from_secs(self.feed.message_timeout_secs)
    }

    pub fn max_data_age(&self) -> Duration {
        Duration::from_millis(self.detect.max_data_age_ms)
    }
}
