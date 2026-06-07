use secrecy::{ExposeSecret, SecretString};
#[cfg(feature = "aws")]
use serde::Deserialize;
use std::env;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("missing environment variable: {0}")]
    MissingEnv(String),
    #[error("AWS Secrets Manager error: {0}")]
    Aws(String),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct ExchangeCredentials {
    pub api_key: SecretString,
    pub secret_key: SecretString,
}

impl std::fmt::Debug for ExchangeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeCredentials")
            .field("api_key", &"***")
            .field("secret_key", &"***")
            .finish()
    }
}

impl ExchangeCredentials {
    pub fn new(api_key: String, secret_key: String) -> Self {
        Self {
            api_key: SecretString::new(api_key.into_boxed_str()),
            secret_key: SecretString::new(secret_key.into_boxed_str()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub binance: ExchangeCredentials,
    pub okx: Option<ExchangeCredentials>,
    pub bybit: Option<ExchangeCredentials>,
}

impl Config {
    pub fn binance_api_key(&self) -> &str {
        self.binance.api_key.expose_secret()
    }

    pub fn binance_secret_key(&self) -> &str {
        self.binance.secret_key.expose_secret()
    }
}

impl Config {
    /// Load configuration from the environment (local development).
    ///
    /// Reads `.env` file (or `env_file` path if provided) and falls back
    /// to process environment variables.
    pub fn from_env(env_file: Option<&str>) -> Result<Self, ConfigError> {
        match env_file {
            Some(path) => {
                let _ = dotenvy::from_filename(path);
            }
            None => {
                let _ = dotenvy::dotenv();
            }
        }

        let binance = ExchangeCredentials::new(
            env::var("BINANCE_API_KEY")
                .map_err(|_| ConfigError::MissingEnv("BINANCE_API_KEY".into()))?,
            env::var("BINANCE_SECRET_KEY")
                .map_err(|_| ConfigError::MissingEnv("BINANCE_SECRET_KEY".into()))?,
        );

        let okx = load_optional("OKX_API_KEY", "OKX_SECRET_KEY");
        let bybit = load_optional("BYBIT_API_KEY", "BYBIT_SECRET_KEY");

        Ok(Self { binance, okx, bybit })
    }

    /// Load configuration from AWS Secrets Manager (production).
    #[cfg(feature = "aws")]
    pub async fn from_aws(secret_name: &str) -> Result<Self, ConfigError> {
        let aws_config = aws_config::load_from_env().await;
        let client = aws_sdk_secretsmanager::Client::new(&aws_config);

        let resp = client
            .get_secret_value()
            .secret_id(secret_name)
            .send()
            .await
            .map_err(|e| ConfigError::Aws(e.to_string()))?;

        let secret_string = resp
            .secret_string()
            .ok_or_else(|| ConfigError::Aws("secret string is empty".into()))?;

        let parsed: AwsSecretFormat = serde_json::from_str(secret_string)?;

        Ok(Self {
            binance: ExchangeCredentials::new(parsed.binance_api_key, parsed.binance_secret_key),
            okx: parsed.okx_api_key.zip(parsed.okx_secret_key).map(
                |(key, secret)| ExchangeCredentials::new(key, secret),
            ),
            bybit: parsed.bybit_api_key.zip(parsed.bybit_secret_key).map(
                |(key, secret)| ExchangeCredentials::new(key, secret),
            ),
        })
    }

    /// Export credentials as environment variables for adapter consumption.
    ///
    /// Required because `of_adapters::AdapterConfig` reads credentials via
    /// `std::env::var` using the env var names in `CredentialsRef`.
    pub fn export_env(&self) {
        env::set_var("BINANCE_API_KEY", self.binance.api_key.expose_secret());
        env::set_var("BINANCE_SECRET_KEY", self.binance.secret_key.expose_secret());
        if let Some(okx) = &self.okx {
            env::set_var("OKX_API_KEY", okx.api_key.expose_secret());
            env::set_var("OKX_SECRET_KEY", okx.secret_key.expose_secret());
        }
        if let Some(bybit) = &self.bybit {
            env::set_var("BYBIT_API_KEY", bybit.api_key.expose_secret());
            env::set_var("BYBIT_SECRET_KEY", bybit.secret_key.expose_secret());
        }
    }
}

fn load_optional(api_key_var: &str, secret_key_var: &str) -> Option<ExchangeCredentials> {
    let api_key = env::var(api_key_var).ok()?;
    let secret_key = env::var(secret_key_var).ok()?;
    Some(ExchangeCredentials::new(api_key, secret_key))
}

#[cfg(feature = "aws")]
#[derive(Deserialize)]
struct AwsSecretFormat {
    binance_api_key: String,
    binance_secret_key: String,
    #[serde(default)]
    okx_api_key: Option<String>,
    #[serde(default)]
    okx_secret_key: Option<String>,
    #[serde(default)]
    bybit_api_key: Option<String>,
    #[serde(default)]
    bybit_secret_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_redacted() {
        let creds = ExchangeCredentials::new("key123".into(), "secret456".into());
        let debug = format!("{:?}", creds);
        assert!(!debug.contains("key123"), "Debug exposed api_key");
        assert!(!debug.contains("secret456"), "Debug exposed secret_key");
        assert!(debug.contains("***"), "Debug missing redaction marker");
    }

    #[test]
    fn test_config_debug_redacted() {
        let cfg = Config {
            binance: ExchangeCredentials::new("my-api-key".into(), "my-secret".into()),
            okx: None,
            bybit: None,
        };
        let debug = format!("{:?}", cfg);
        assert!(!debug.contains("my-api-key"));
        assert!(!debug.contains("my-secret"));
        assert!(debug.contains("***"));
    }
}
