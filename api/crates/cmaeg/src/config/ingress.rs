//! Callback transport bounds and key-cache lifetime.
use super::{positive_duration, ConfigError};
use std::{env, time::Duration};
pub const CMAEG_JWKS_TTL_MS: &str = "CMAEG_JWKS_TTL_MS";
pub const CMAEG_INGRESS_MAX_BODY: &str = "CMAEG_INGRESS_MAX_BODY";
pub const CMAEG_INGRESS_INFLIGHT: &str = "CMAEG_INGRESS_INFLIGHT";
#[derive(Debug, Clone)]
pub struct IngressConfig {
    pub jwks_ttl: Duration,
    pub max_body: usize,
    pub max_inflight: usize,
}
impl Default for IngressConfig {
    fn default() -> Self {
        Self {
            jwks_ttl: Duration::from_secs(600),
            max_body: 1_048_576,
            max_inflight: 256,
        }
    }
}
impl IngressConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            jwks_ttl: positive_duration(CMAEG_JWKS_TTL_MS, 600_000)?,
            max_body: count(CMAEG_INGRESS_MAX_BODY, 1_048_576, 16 * 1_048_576)?,
            max_inflight: count(
                CMAEG_INGRESS_INFLIGHT,
                256,
                tokio::sync::Semaphore::MAX_PERMITS,
            )?,
        })
    }
}
fn count(name: &str, default: usize, max: usize) -> Result<usize, ConfigError> {
    env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse()
        .ok()
        .filter(|value| *value > 0 && *value <= max)
        .ok_or_else(|| ConfigError::InvalidManagement {
            detail: format!("{name} must be within 1..={max}"),
        })
}
