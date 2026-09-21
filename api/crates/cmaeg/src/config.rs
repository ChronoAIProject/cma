//! Environment configuration for the CMA Event Gateway.

use std::{env, net::SocketAddr};

use crate::providers::ProviderRegistry;

/// The listener address used by CMAEG.
pub const CMAEG_LISTEN: &str = "CMAEG_LISTEN";
pub const CMAEG_PROVIDERS: &str = "CMAEG_PROVIDERS";
pub const CMAEG_PROVIDER_CONNECT_TIMEOUT_MS: &str = "CMAEG_PROVIDER_CONNECT_TIMEOUT_MS";
pub const CMAEG_PROVIDER_REQUEST_TIMEOUT_MS: &str = "CMAEG_PROVIDER_REQUEST_TIMEOUT_MS";
pub const CMAEG_PROVIDER_MAX_INFLIGHT: &str = "CMAEG_PROVIDER_MAX_INFLIGHT";

/// Configuration errors that prevent the service from binding safely.
#[derive(Debug)]
pub enum ConfigError {
    /// The configured listener was not a valid socket address.
    InvalidListen {
        value: String,
        source: std::net::AddrParseError,
    },
    /// The provider registry JSON did not satisfy the outbound-call policy.
    InvalidProviders { detail: String },
    /// A duration setting was not a positive bounded integer.
    InvalidDuration { name: &'static str, value: String },
    /// The global provider concurrency cap was invalid.
    InvalidMaxInflight { value: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListen { value, source } => {
                write!(formatter, "{CMAEG_LISTEN}={value:?} is invalid: {source}")
            }
            Self::InvalidProviders { detail } => {
                write!(formatter, "{CMAEG_PROVIDERS} is invalid: {detail}")
            }
            Self::InvalidDuration { name, value } => {
                write!(
                    formatter,
                    "{name}={value:?} is invalid: expected a positive millisecond value"
                )
            }
            Self::InvalidMaxInflight { value } => {
                write!(formatter, "{CMAEG_PROVIDER_MAX_INFLIGHT}={value:?} is invalid: expected a positive integer")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Process configuration loaded once at startup.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address on which the HTTP service accepts requests.
    pub listen: SocketAddr,
    /// Providers explicitly allowed to receive outbound CMAEG requests.
    pub providers: ProviderRegistry,
    /// TCP/TLS connection establishment timeout for provider clients.
    pub provider_connect_timeout: std::time::Duration,
    /// Provider request timeout for non-streaming setup calls.
    pub provider_request_timeout: std::time::Duration,
    /// Deployment-wide upper bound applied to each provider semaphore.
    pub provider_max_inflight: usize,
}

impl Config {
    /// Read configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = env::var(CMAEG_LISTEN).unwrap_or_else(|_| "0.0.0.0:8090".to_owned());
        let listen = value
            .parse()
            .map_err(|source| ConfigError::InvalidListen { value, source })?;
        let providers_json = env::var(CMAEG_PROVIDERS).unwrap_or_else(|_| "[]".to_owned());
        let providers = ProviderRegistry::parse(&providers_json)?;
        let provider_connect_timeout = positive_duration(CMAEG_PROVIDER_CONNECT_TIMEOUT_MS, 2_000)?;
        let provider_request_timeout =
            positive_duration(CMAEG_PROVIDER_REQUEST_TIMEOUT_MS, 30_000)?;
        let provider_max_inflight = env::var(CMAEG_PROVIDER_MAX_INFLIGHT)
            .unwrap_or_else(|_| "256".to_owned())
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| ConfigError::InvalidMaxInflight {
                value: env::var(CMAEG_PROVIDER_MAX_INFLIGHT).unwrap_or_else(|_| "256".to_owned()),
            })?;
        Ok(Self {
            listen,
            providers,
            provider_connect_timeout,
            provider_request_timeout,
            provider_max_inflight,
        })
    }
}

fn positive_duration(
    name: &'static str,
    default_ms: u64,
) -> Result<std::time::Duration, ConfigError> {
    let value = env::var(name).unwrap_or_else(|_| default_ms.to_string());
    let milliseconds = value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ConfigError::InvalidDuration {
            name,
            value: value.clone(),
        })?;
    Ok(std::time::Duration::from_millis(milliseconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_the_gateway_bootable_without_provider_calls() {
        let config = Config::from_env().unwrap();
        assert_eq!(config.listen.port(), 8090);
        assert!(config.providers.is_empty());
        assert_eq!(config.provider_max_inflight, 256);
    }
}
