//! Short-lived channel cache and bounded cold-restore settings.
use super::{positive_duration, ConfigError};
use std::{env, num::NonZeroU32, time::Duration};

pub const CMAEG_CHANNEL_CACHE_TTL_MS: &str = "CMAEG_CHANNEL_CACHE_TTL_MS";
pub const CMAEG_CHANNEL_NEGATIVE_TTL_MS: &str = "CMAEG_CHANNEL_NEGATIVE_TTL_MS";
pub const CMAEG_CHANNEL_CACHE_MAX: &str = "CMAEG_CHANNEL_CACHE_MAX";
pub const CMAEG_RESTORE_RATE_PER_MINUTE: &str = "CMAEG_RESTORE_RATE_PER_MINUTE";
pub const CMAEG_RESTORE_TIMEOUT_MS: &str = "CMAEG_RESTORE_TIMEOUT_MS";

#[derive(Debug, Clone)]
pub struct ChannelCacheConfig {
    pub ttl: Duration,
    pub negative_ttl: Duration,
    pub max_entries: usize,
    pub restores_per_minute: NonZeroU32,
    pub restore_timeout: Duration,
}
impl Default for ChannelCacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(60),
            negative_ttl: Duration::from_secs(10),
            max_entries: 50_000,
            restores_per_minute: NonZeroU32::new(10).unwrap(),
            restore_timeout: Duration::from_millis(800),
        }
    }
}
impl ChannelCacheConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            ttl: positive_duration(CMAEG_CHANNEL_CACHE_TTL_MS, 60_000)?,
            negative_ttl: positive_duration(CMAEG_CHANNEL_NEGATIVE_TTL_MS, 10_000)?,
            max_entries: positive_count(CMAEG_CHANNEL_CACHE_MAX, 50_000)?,
            restores_per_minute: NonZeroU32::new(
                u32::try_from(positive_count(CMAEG_RESTORE_RATE_PER_MINUTE, 10)?)
                    .map_err(|_| invalid(CMAEG_RESTORE_RATE_PER_MINUTE))?,
            )
            .ok_or_else(|| invalid(CMAEG_RESTORE_RATE_PER_MINUTE))?,
            restore_timeout: positive_duration(CMAEG_RESTORE_TIMEOUT_MS, 800)?,
        })
    }
}
fn positive_count(name: &'static str, default: usize) -> Result<usize, ConfigError> {
    env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<usize>()
        .ok()
        .filter(|count| *count > 0)
        .ok_or_else(|| invalid(name))
}
fn invalid(name: &str) -> ConfigError {
    ConfigError::InvalidManagement {
        detail: format!("{name} must be a positive bounded integer"),
    }
}
