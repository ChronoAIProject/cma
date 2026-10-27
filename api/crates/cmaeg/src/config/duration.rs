//! Positive duration parsing shared by environment configuration.
use super::ConfigError;
use std::env;

pub(super) fn positive_duration(
    name: &'static str,
    default_ms: u64,
) -> Result<std::time::Duration, ConfigError> {
    let value = env::var(name).unwrap_or_else(|_| default_ms.to_string());
    let milliseconds = value
        .parse::<u64>()
        .ok()
        .filter(|value| {
            *value > 0
                && std::time::Instant::now()
                    .checked_add(std::time::Duration::from_millis(*value))
                    .is_some()
        })
        .ok_or_else(|| ConfigError::InvalidDuration {
            name,
            value: value.clone(),
        })?;
    Ok(std::time::Duration::from_millis(milliseconds))
}
