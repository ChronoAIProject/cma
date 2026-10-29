//! Reader-first native Discord activation, separate from source authoring.
use super::{routing::RoutingConfig, ConfigError};
use std::{collections::BTreeSet, env};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeDiscordConfig {
    pub enabled: bool,
    /// Discover explicit native sources in the current sealed channel index.
    pub discovery_enabled: bool,
    /// Emit versioned sticker/embed/poll snapshots after compatible readers drain.
    pub rich_content_enabled: bool,
    pub records: BTreeSet<String>,
}
pub(super) fn config_from_env(routing: &RoutingConfig) -> Result<NativeDiscordConfig, ConfigError> {
    let value = env::var("CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS")
        .map(Some)
        .or_else(|error| match error {
            env::VarError::NotPresent => Ok(None),
            _ => Err(ConfigError::Routing(
                "CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS must be UTF-8".into(),
            )),
        })?;
    let config = NativeDiscordConfig {
        enabled: super::routing::write_flag("CMAEG_NATIVE_DISCORD_ENABLED")?,
        discovery_enabled: super::routing::write_flag("CMAEG_NATIVE_DISCORD_DISCOVERY_ENABLED")?,
        rich_content_enabled: super::routing::write_flag(
            "CMAEG_NATIVE_DISCORD_RICH_CONTENT_ENABLED",
        )?,
        records: parse_records(value.as_deref())?,
    };
    config.validate(routing)?;
    Ok(config)
}
impl NativeDiscordConfig {
    pub fn validate(&self, routing: &RoutingConfig) -> Result<(), ConfigError> {
        if self.records.len() > 64 || self.records.iter().any(|record| !canonical(record)) {
            return Err(ConfigError::Routing(
                "native Discord requires at most 64 canonical UUID v4 channel records".into(),
            ));
        }
        if (self.discovery_enabled || self.rich_content_enabled)
            && (!self.enabled || !routing.recovery_enabled)
        {
            return Err(ConfigError::Routing(
                "native Discord discovery and rich content require native runtime and routed recovery".into(),
            ));
        }
        if self.enabled
            && !self.records.is_empty()
            && (!routing.recovery_enabled || !self.records.is_subset(&routing.execution_records))
        {
            return Err(ConfigError::Routing("native Discord records require routed recovery and explicit routed record selection".into()));
        }
        Ok(())
    }
}
fn canonical(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
}
fn parse_records(value: Option<&str>) -> Result<BTreeSet<String>, ConfigError> {
    let records: BTreeSet<_> = value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    if records.len() > 64 || records.iter().any(|value| !canonical(value)) {
        return Err(ConfigError::Routing("CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS requires at most 64 explicit canonical UUID v4 records".into()));
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rich_content_writer_is_independent_default_off_and_requires_native_recovery() {
        let mut config = NativeDiscordConfig::default();
        assert!(!config.rich_content_enabled);
        config.rich_content_enabled = true;
        let mut routing = RoutingConfig::default();
        assert!(config.validate(&routing).is_err());
        config.enabled = true;
        assert!(config.validate(&routing).is_err());
        routing.recovery_enabled = true;
        config.validate(&routing).unwrap();
        assert!(!config.discovery_enabled);
        assert!(config.records.is_empty());
    }
    #[test]
    fn discovery_needs_native_runtime_and_recovery_but_no_http_record_expansion() {
        let mut config = NativeDiscordConfig {
            discovery_enabled: true,
            ..Default::default()
        };
        let mut routing = RoutingConfig::default();
        assert!(config.validate(&routing).is_err());
        config.enabled = true;
        assert!(config.validate(&routing).is_err());
        routing.recovery_enabled = true;
        config.validate(&routing).unwrap();
        assert!(routing.execution_records.is_empty());
    }
    #[test]
    fn activation_is_explicit_bounded_and_requires_durable_recovery() {
        assert!(!NativeDiscordConfig::default().enabled);
        assert!(parse_records(None).unwrap().is_empty());
        for value in ["*", "not-a-record", "11111111-1111-1111-8111-111111111111"] {
            assert!(parse_records(Some(value)).is_err());
        }
        let record = "11111111-1111-4111-8111-111111111111";
        let config = NativeDiscordConfig {
            enabled: true,
            discovery_enabled: false,
            rich_content_enabled: false,
            records: parse_records(Some(record)).unwrap(),
        };
        let mut routing = RoutingConfig::default();
        assert!(config.validate(&routing).is_err());
        routing.recovery_enabled = true;
        assert!(config.validate(&routing).is_err());
        routing.execution_records.insert(record.into());
        config.validate(&routing).unwrap();
    }
}
