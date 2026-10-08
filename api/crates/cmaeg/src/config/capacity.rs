//! Routed turns share global provider permits and the existing delivery engine.
//! Bound each record and owner before admitting the amplified work.
use super::ConfigError;
use std::env;

pub const CMAEG_ROUTED_CHANNEL_MAX_TURNS: &str = "CMAEG_ROUTED_CHANNEL_MAX_TURNS";
pub const CMAEG_ROUTED_OWNER_MAX_TURNS: &str = "CMAEG_ROUTED_OWNER_MAX_TURNS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutedCapacityConfig {
    pub per_channel: u32,
    pub per_owner: u32,
}
impl Default for RoutedCapacityConfig {
    fn default() -> Self {
        Self {
            per_channel: 4,
            per_owner: 16,
        }
    }
}
impl RoutedCapacityConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        let channel = read(CMAEG_ROUTED_CHANNEL_MAX_TURNS)?;
        let owner = read(CMAEG_ROUTED_OWNER_MAX_TURNS)?;
        Self::parse(channel.as_deref(), owner.as_deref())
    }
    fn parse(channel: Option<&str>, owner: Option<&str>) -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let per_channel = bounded(
            CMAEG_ROUTED_CHANNEL_MAX_TURNS,
            channel,
            defaults.per_channel,
            256,
        )?;
        let per_owner = bounded(
            CMAEG_ROUTED_OWNER_MAX_TURNS,
            owner,
            defaults.per_owner,
            1024,
        )?;
        if per_channel > per_owner {
            tracing::error!(
                key = CMAEG_ROUTED_OWNER_MAX_TURNS,
                channel_key = CMAEG_ROUTED_CHANNEL_MAX_TURNS,
                "owner turn cap must cover channel turn cap"
            );
            return Err(ConfigError::Routing(format!(
                "{CMAEG_ROUTED_OWNER_MAX_TURNS} must be at least {CMAEG_ROUTED_CHANNEL_MAX_TURNS}"
            )));
        }
        tracing::info!(per_channel, per_owner, "routed turn capacity configured");
        Ok(Self {
            per_channel,
            per_owner,
        })
    }
}
fn read(key: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            tracing::error!(key, "routed capacity must be valid UTF-8");
            Err(ConfigError::Routing(format!("{key} must be valid UTF-8")))
        }
    }
}
fn bounded(
    key: &'static str,
    value: Option<&str>,
    default: u32,
    max: u32,
) -> Result<u32, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|number| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && (1..=max).contains(number)
        })
        .ok_or_else(|| {
            tracing::error!(key, min = 1, max, "invalid routed turn capacity");
            ConfigError::Routing(format!("{key} must be an integer in 1..={max}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_inclusive_bounds() {
        assert_eq!(
            RoutedCapacityConfig::parse(None, None).unwrap(),
            RoutedCapacityConfig::default()
        );
        for (channel, owner) in [("1", "1"), ("256", "1024"), ("256", "256")] {
            let actual = RoutedCapacityConfig::parse(Some(channel), Some(owner)).unwrap();
            assert_eq!(actual.per_channel.to_string(), channel);
            assert_eq!(actual.per_owner.to_string(), owner);
        }
    }
    #[test]
    fn malformed_and_out_of_range_values_name_only_the_key() {
        for value in [
            "",
            "0",
            "257",
            "-1",
            "+1",
            " 4",
            "4 ",
            "4294967296",
            "private-config-sentinel",
        ] {
            let error = RoutedCapacityConfig::parse(Some(value), Some("1024"))
                .unwrap_err()
                .to_string();
            assert_eq!(error, "routing configuration is invalid: CMAEG_ROUTED_CHANNEL_MAX_TURNS must be an integer in 1..=256");
        }
        for value in ["", "0", "1025", "-1", "1.5", "private-config-sentinel"] {
            let error = RoutedCapacityConfig::parse(Some("1"), Some(value))
                .unwrap_err()
                .to_string();
            assert_eq!(error, "routing configuration is invalid: CMAEG_ROUTED_OWNER_MAX_TURNS must be an integer in 1..=1024");
        }
    }
    #[test]
    fn owner_must_cover_channel_even_with_one_default() {
        for (channel, owner) in [
            (Some("17"), None),
            (None, Some("3")),
            (Some("8"), Some("7")),
        ] {
            let error = RoutedCapacityConfig::parse(channel, owner)
                .unwrap_err()
                .to_string();
            assert!(error.contains(
                "CMAEG_ROUTED_OWNER_MAX_TURNS must be at least CMAEG_ROUTED_CHANNEL_MAX_TURNS"
            ));
        }
    }
}
