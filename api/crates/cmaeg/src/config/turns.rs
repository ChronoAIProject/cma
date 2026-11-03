use super::{positive_duration, ConfigError};
use std::{
    env,
    time::{Duration, Instant},
};
pub const CMAEG_ACCEPT_BUDGET_MS: &str = "CMAEG_ACCEPT_BUDGET_MS";
pub const CMAEG_TURN_TIMEOUT_MS: &str = "CMAEG_TURN_TIMEOUT_MS";
pub const CMAEG_DRAIN_SECONDS: &str = "CMAEG_DRAIN_SECONDS";
pub const CMAEG_BUSY_RETRY_MAX_MS: &str = "CMAEG_BUSY_RETRY_MAX_MS";
#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub accept_budget: Duration,
    pub turn_timeout: Duration,
    pub drain: Duration,
    pub busy_retry_max: Duration,
}
impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            accept_budget: Duration::from_secs(20),
            turn_timeout: Duration::from_secs(600),
            drain: Duration::from_secs(660),
            busy_retry_max: Duration::from_secs(180),
        }
    }
}
impl TurnConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        let seconds = env::var(CMAEG_DRAIN_SECONDS)
            .unwrap_or_else(|_| "660".into())
            .parse::<u64>()
            .ok()
            .filter(|value| {
                *value > 0
                    && Instant::now()
                        .checked_add(Duration::from_secs(*value))
                        .is_some()
            })
            .ok_or_else(|| ConfigError::InvalidManagement {
                detail: format!("{CMAEG_DRAIN_SECONDS} must be positive and representable"),
            })?;
        Ok(Self {
            accept_budget: positive_duration(CMAEG_ACCEPT_BUDGET_MS, 20000)?,
            turn_timeout: positive_duration(CMAEG_TURN_TIMEOUT_MS, 600000)?,
            drain: Duration::from_secs(seconds),
            busy_retry_max: positive_duration(CMAEG_BUSY_RETRY_MAX_MS, 180000)?,
        })
    }
}
