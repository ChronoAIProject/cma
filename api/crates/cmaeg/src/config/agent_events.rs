//! Agent hand-off budgets are validated even while the feature is disabled.
use super::ConfigError;
use std::{env, time::Duration};

pub const CMAEG_AGENT_EVENTS_ENABLED: &str = "CMAEG_AGENT_EVENTS_ENABLED";
pub const CMAEG_AGENT_EVENTS_RATE_PER_MINUTE: &str = "CMAEG_AGENT_EVENTS_RATE_PER_MINUTE";
pub const CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE: &str =
    "CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE";
pub const CMAEG_AGENT_EVENTS_ACCEPT_MS: &str = "CMAEG_AGENT_EVENTS_ACCEPT_MS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentEventsConfig {
    pub enabled: bool,
    pub rate_per_minute: u32,
    pub owner_rate_per_minute: u32,
    /// Measured from RequestStart; body reads and authorization spend this budget.
    pub accept_budget: Duration,
}

impl Default for AgentEventsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rate_per_minute: 600,
            owner_rate_per_minute: 120,
            accept_budget: Duration::from_millis(5_000),
        }
    }
}

impl AgentEventsConfig {
    pub(super) fn from_env(multi_consumer: bool) -> Result<Self, ConfigError> {
        Self::from_lookup(read, multi_consumer)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&'static str) -> Result<Option<String>, ConfigError>,
        multi_consumer: bool,
    ) -> Result<Self, ConfigError> {
        let enabled = lookup(CMAEG_AGENT_EVENTS_ENABLED)?;
        let rate = lookup(CMAEG_AGENT_EVENTS_RATE_PER_MINUTE)?;
        let owner = lookup(CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE)?;
        let accept = lookup(CMAEG_AGENT_EVENTS_ACCEPT_MS)?;
        let config = Self::parse(
            enabled.as_deref(),
            rate.as_deref(),
            owner.as_deref(),
            accept.as_deref(),
        )?;
        if config.enabled && !multi_consumer {
            tracing::warn!(
                "agent-event publishing remains unlisted and disabled while CMAEG_MULTI_CONSUMER_ENABLED is false"
            );
        }
        tracing::info!(
            enabled = config.enabled,
            multi_consumer,
            rate_per_minute = config.rate_per_minute,
            owner_rate_per_minute = config.owner_rate_per_minute,
            accept_ms = config.accept_budget.as_millis(),
            "agent-event budgets configured"
        );
        Ok(config)
    }

    fn parse(
        enabled: Option<&str>,
        rate: Option<&str>,
        owner: Option<&str>,
        accept: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let enabled = match enabled {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => return Err(invalid(CMAEG_AGENT_EVENTS_ENABLED, "must be true or false")),
        };
        let defaults = Self::default();
        let rate_per_minute = bounded(
            CMAEG_AGENT_EVENTS_RATE_PER_MINUTE,
            rate,
            defaults.rate_per_minute,
            10,
            6_000,
        )?;
        let owner_rate_per_minute = bounded(
            CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE,
            owner,
            defaults.owner_rate_per_minute,
            1,
            6_000,
        )?;
        let accept_ms = bounded(CMAEG_AGENT_EVENTS_ACCEPT_MS, accept, 5_000, 500, 7_000)?;
        if owner_rate_per_minute > rate_per_minute {
            return Err(invalid(
                CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE,
                &format!("must not exceed {CMAEG_AGENT_EVENTS_RATE_PER_MINUTE}"),
            ));
        }
        Ok(Self {
            enabled,
            rate_per_minute,
            owner_rate_per_minute,
            accept_budget: Duration::from_millis(u64::from(accept_ms)),
        })
    }
}

fn read(key: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(invalid(key, "must be valid UTF-8")),
    }
}

fn bounded(
    key: &'static str,
    value: Option<&str>,
    default: u32,
    min: u32,
    max: u32,
) -> Result<u32, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|n| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && (min..=max).contains(n)
        })
        .ok_or_else(|| invalid(key, &format!("must be an integer in {min}..={max}")))
}

fn invalid(key: &'static str, reason: &str) -> ConfigError {
    // Operator input may be pasted from a secret by mistake. Name the key only.
    tracing::error!(key, reason, "invalid agent-event configuration");
    ConfigError::Routing(format!("{key} {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_inclusive_bounds() {
        assert_eq!(
            AgentEventsConfig::parse(None, None, None, None).unwrap(),
            AgentEventsConfig::default()
        );
        for (rate, owner, accept) in [("10", "1", "500"), ("6000", "6000", "7000")] {
            let config =
                AgentEventsConfig::parse(Some("true"), Some(rate), Some(owner), Some(accept))
                    .unwrap();
            assert!(config.enabled);
            assert_eq!(config.rate_per_minute.to_string(), rate);
            assert_eq!(config.owner_rate_per_minute.to_string(), owner);
            assert_eq!(config.accept_budget.as_millis().to_string(), accept);
        }
    }

    #[test]
    fn invalid_switch_and_numeric_values_never_echo_input() {
        for value in ["", "True", "1", " true", "private-config-sentinel"] {
            let error = AgentEventsConfig::parse(Some(value), None, None, None)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "routing configuration is invalid: CMAEG_AGENT_EVENTS_ENABLED must be true or false");
        }
        for (field, values) in [
            (CMAEG_AGENT_EVENTS_RATE_PER_MINUTE, vec!["9", "6001"]),
            (CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE, vec!["0", "6001"]),
            (CMAEG_AGENT_EVENTS_ACCEPT_MS, vec!["499", "7001"]),
        ] {
            for value in values.into_iter().chain([
                "",
                "-1",
                "+1",
                " 600",
                "600 ",
                "1.5",
                "4294967296",
                "private-config-sentinel",
            ]) {
                let result = AgentEventsConfig::from_lookup(
                    |key| Ok((key == field).then(|| value.to_owned())),
                    false,
                );
                let error = result.unwrap_err().to_string();
                assert!(error.contains(field));
                assert!(!error.contains("private-config-sentinel"));
            }
        }
    }

    #[test]
    fn owner_cannot_exceed_the_global_budget_even_while_disabled() {
        for (rate, owner) in [
            (Some("119"), None),
            (None, Some("601")),
            (Some("10"), Some("11")),
        ] {
            let error = AgentEventsConfig::parse(Some("false"), rate, owner, None)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "routing configuration is invalid: CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE must not exceed CMAEG_AGENT_EVENTS_RATE_PER_MINUTE");
        }
    }

    #[test]
    fn lookup_errors_propagate_and_an_inactive_switch_still_loads() {
        let error = AgentEventsConfig::from_lookup(
            |key| {
                if key == CMAEG_AGENT_EVENTS_ACCEPT_MS {
                    Err(invalid(key, "must be valid UTF-8"))
                } else {
                    Ok(None)
                }
            },
            false,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "routing configuration is invalid: CMAEG_AGENT_EVENTS_ACCEPT_MS must be valid UTF-8"
        );
        let config = AgentEventsConfig::from_lookup(
            |key| Ok((key == CMAEG_AGENT_EVENTS_ENABLED).then(|| "true".into())),
            false,
        )
        .unwrap();
        assert!(config.enabled);
        assert_eq!(config.accept_budget, Duration::from_secs(5));
    }
}
