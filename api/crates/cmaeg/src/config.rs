//! Environment configuration for the CMA Event Gateway.

use std::{env, net::SocketAddr};

/// The listener address used by CMAEG.
pub const CMAEG_LISTEN: &str = "CMAEG_LISTEN";

/// Configuration errors that prevent the service from binding safely.
#[derive(Debug)]
pub enum ConfigError {
    /// The configured listener was not a valid socket address.
    InvalidListen {
        value: String,
        source: std::net::AddrParseError,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListen { value, source } => {
                write!(formatter, "{CMAEG_LISTEN}={value:?} is invalid: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Process configuration loaded once at startup.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Address on which the HTTP service accepts requests.
    pub listen: SocketAddr,
}

impl Config {
    /// Read configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let value = env::var(CMAEG_LISTEN).unwrap_or_else(|_| "0.0.0.0:8090".to_owned());
        let listen = value
            .parse()
            .map_err(|source| ConfigError::InvalidListen { value, source })?;
        Ok(Self { listen })
    }
}
