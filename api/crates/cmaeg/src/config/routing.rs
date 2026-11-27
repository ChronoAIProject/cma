//! Multi-consumer routing settings: which providers' agents answer non-human
//! actors (custom events and hand-offs) unless a consumer says otherwise.
//!
//! The actor-kind capability is per-consumer data with a slug default, and
//! the gateway never reads provider-specific profile fields to decide it. The
//! default is operator configuration under its own key rather than a field of
//! a `CMAEG_PROVIDERS` entry because those entries are `deny_unknown_fields`:
//! an older image reading a newer ConfigMap would refuse to start.
use super::ConfigError;
use crate::providers::ProviderRegistry;
use cmaeg_core::ids::ProviderSlug;
use std::{collections::BTreeSet, env};

pub const CMAEG_NON_HUMAN_PROVIDERS: &str = "CMAEG_NON_HUMAN_PROVIDERS";
pub const CMAEG_ROUTED_CHANNEL_RECORDS: &str = "CMAEG_ROUTED_CHANNEL_RECORDS";
pub const CMAEG_ROUTED_RECOVERY_ENABLED: &str = "CMAEG_ROUTED_RECOVERY_ENABLED";
pub const CMAEG_ROUTED_ADMISSION_ENABLED: &str = "CMAEG_ROUTED_ADMISSION_ENABLED";

/// The one provider whose agents are known to answer custom events and
/// hand-offs.
const DEFAULT_NON_HUMAN_PROVIDER: &str = "cma_codex";
/// More slugs than any deployment configures providers; a longer list is an
/// operator mistake rather than a policy.
const MAX_NON_HUMAN_PROVIDERS: usize = 8;

/// Routing settings loaded after, and validated against, the provider
/// registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingConfig {
    /// Keep enabled while any durably admitted target still needs recovery.
    pub recovery_enabled: bool,
    /// Collect bounded first-write observations for routed events.
    pub provider_start_measurement_enabled: bool,
    /// Reader-first writer gate for immutable self-service Custom selection.
    pub custom_durable_execution_enabled: bool,
    /// Keep the generation reader active after any binding migrates.
    pub binding_preparation_enabled: bool,
    /// Only newly admitted ordinary events can capture replacement opt-in.
    pub binding_replacement_bindings: BTreeSet<String>,
    /// Reader-first gate for new native reply-context snapshots.
    pub quoted_context_enabled: bool,
    /// Writer gate; the optional indexed reader keeps existing obligations alive.
    pub media_continuations_enabled: bool,
    pub accepted_answer_receipts_enabled: bool,
    /// Pauses new routed admissions without abandoning accepted work or retries.
    pub admission_enabled: bool,
    /// Explicit rollout selection; an empty set preserves all legacy ingress.
    pub execution_records: BTreeSet<String>,
    /// Provider slugs whose agents answer non-human actors (custom events,
    /// hand-offs) by default. A consumer's `non_human` field overrides this
    /// per consumer.
    pub non_human_providers: BTreeSet<String>,
}

/// Read every routing setting; the non-human set is checked against the
/// registry, so this can only run after the registry is loaded.
pub(super) fn config_from_env(providers: &ProviderRegistry) -> Result<RoutingConfig, ConfigError> {
    let value = match env::var(CMAEG_NON_HUMAN_PROVIDERS) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(env::VarError::NotUnicode(_)) => {
            return Err(ConfigError::Routing(format!(
                "{CMAEG_NON_HUMAN_PROVIDERS} must be valid UTF-8"
            )))
        }
    };
    let config = RoutingConfig {
        custom_durable_execution_enabled: write_flag("CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED")?,
        binding_preparation_enabled: write_flag("CMAEG_BINDING_PREPARATION_ENABLED")?,
        binding_replacement_bindings: binding_replacements(
            env::var("CMAEG_BINDING_REPLACEMENT_BINDINGS")
                .map(Some)
                .or_else(|error| match error {
                    env::VarError::NotPresent => Ok(None),
                    _ => Err(ConfigError::Routing(
                        "CMAEG_BINDING_REPLACEMENT_BINDINGS must be valid UTF-8".into(),
                    )),
                })?
                .as_deref(),
        )?,
        quoted_context_enabled: write_flag("CMAEG_NATIVE_QUOTED_CONTEXT_ENABLED")?,
        media_continuations_enabled: write_flag("CMAEG_MEDIA_CONTINUATIONS_ENABLED")?,
        accepted_answer_receipts_enabled: write_flag("CMAEG_ACCEPTED_ANSWER_RECEIPTS_ENABLED")?,
        recovery_enabled: write_flag(CMAEG_ROUTED_RECOVERY_ENABLED)?,
        provider_start_measurement_enabled: write_flag("CMAEG_PROVIDER_START_MEASUREMENT_ENABLED")?,
        admission_enabled: write_flag(CMAEG_ROUTED_ADMISSION_ENABLED)?,
        execution_records: execution_records(
            env::var(CMAEG_ROUTED_CHANNEL_RECORDS)
                .map(Some)
                .or_else(|error| match error {
                    env::VarError::NotPresent => Ok(None),
                    _ => Err(ConfigError::Routing(format!(
                        "{CMAEG_ROUTED_CHANNEL_RECORDS} must be valid UTF-8"
                    ))),
                })?
                .as_deref(),
        )?,
        non_human_providers: non_human_providers(value.as_deref(), providers)?,
    };
    config.validate()?;
    if config.binding_preparation_enabled
        && providers
            .get(&ProviderSlug::new("cma_codex").expect("constant slug"))
            .is_none_or(|entry| entry.paths.preparation.is_none())
    {
        return Err(ConfigError::Routing(
            "binding preparation requires explicit CMA provider preparation paths".into(),
        ));
    }
    Ok(config)
}

impl RoutingConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !self.recovery_enabled
            && (self.custom_durable_execution_enabled
                || self.admission_enabled
                || self.provider_start_measurement_enabled
                || self.quoted_context_enabled
                || self.media_continuations_enabled
                || self.accepted_answer_receipts_enabled
                || self.binding_preparation_enabled
                || !self.execution_records.is_empty())
        {
            return Err(ConfigError::Routing(format!(
                "{CMAEG_ROUTED_RECOVERY_ENABLED} must be enabled before selecting records or admitting routed events"
            )));
        }
        if !self.binding_preparation_enabled && !self.binding_replacement_bindings.is_empty() {
            return Err(ConfigError::Routing(
                "binding replacements require generation-aware preparation".into(),
            ));
        }
        Ok(())
    }
}

fn binding_replacements(value: Option<&str>) -> Result<BTreeSet<String>, ConfigError> {
    let mut selected = BTreeSet::new();
    for binding in value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if binding.len() > 128
            || !binding
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || cmaeg_core::ids::BindingId::new(binding).is_err()
        {
            return Err(ConfigError::Routing(
                "CMAEG_BINDING_REPLACEMENT_BINDINGS requires explicit binding IDs".into(),
            ));
        }
        selected.insert(binding.to_owned());
        if selected.len() > 256 {
            return Err(ConfigError::Routing(
                "binding replacement selection allows at most 256 bindings".into(),
            ));
        }
    }
    Ok(selected)
}

fn execution_records(value: Option<&str>) -> Result<BTreeSet<String>, ConfigError> {
    let mut records = BTreeSet::new();
    for value in value.unwrap_or_default().split(',').map(str::trim) {
        if value.is_empty() {
            continue;
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || cmaeg_core::ids::RecordId::new(value).is_err()
        {
            return Err(ConfigError::Routing(format!(
                "{CMAEG_ROUTED_CHANNEL_RECORDS} must contain explicit record IDs"
            )));
        }
        records.insert(value.to_owned());
        if records.len() > 64 {
            return Err(ConfigError::Routing(format!(
                "{CMAEG_ROUTED_CHANNEL_RECORDS} allows at most 64 records"
            )));
        }
    }
    Ok(records)
}

/// Read the non-human provider set.
///
/// Unset selects the default only when the registry configures it, so a
/// gateway without that provider still starts. A set value, including the
/// empty string, is strict: every slug must name a configured provider, and
/// repeated slugs collapse.
fn non_human_providers(
    value: Option<&str>,
    registry: &ProviderRegistry,
) -> Result<BTreeSet<String>, ConfigError> {
    let Some(value) = value else {
        return Ok(default_set(registry));
    };
    let mut slugs = BTreeSet::new();
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let slug = ProviderSlug::new(entry)
            .ok()
            .filter(|slug| registry.get(slug).is_some())
            .ok_or_else(|| {
                ConfigError::Routing(format!(
                    "{CMAEG_NON_HUMAN_PROVIDERS} names unknown provider {entry}"
                ))
            })?;
        slugs.insert(slug.as_str().to_owned());
        if slugs.len() > MAX_NON_HUMAN_PROVIDERS {
            return Err(ConfigError::Routing(format!(
                "{CMAEG_NON_HUMAN_PROVIDERS} names more than {MAX_NON_HUMAN_PROVIDERS} providers"
            )));
        }
    }
    tracing::info!(
        key = CMAEG_NON_HUMAN_PROVIDERS,
        providers = slugs.len(),
        "non-human providers configured"
    );
    Ok(slugs)
}

fn default_set(registry: &ProviderRegistry) -> BTreeSet<String> {
    match ProviderSlug::new(DEFAULT_NON_HUMAN_PROVIDER) {
        Ok(slug) if registry.get(&slug).is_some() => BTreeSet::from([slug.as_str().to_owned()]),
        _ => {
            tracing::info!(
                key = CMAEG_NON_HUMAN_PROVIDERS,
                "default non-human provider cma_codex is not configured; consumers answer non-human actors only by their own non_human field"
            );
            BTreeSet::new()
        }
    }
}

pub(super) fn telegram_received_reactions() -> Result<bool, super::ConfigError> {
    write_flag("CMAEG_NATIVE_TELEGRAM_RECEIVED_REACTIONS_ENABLED")
}
pub(super) fn discord_received_reactions() -> Result<bool, super::ConfigError> {
    write_flag("CMAEG_NATIVE_DISCORD_RECEIVED_REACTIONS_ENABLED")
}

/// The GitHub write flag, shared by the server and the offline importer.
pub(crate) fn github_writes_enabled() -> Result<bool, super::ConfigError> {
    write_flag("CMAEG_GITHUB_ENABLED")
}

/// Reader-first authoring gate. Unset is off unless the gate has graduated
/// (see `super::graduated`); an explicit `false` always turns it off.
pub(super) fn write_flag(name: &str) -> Result<bool, super::ConfigError> {
    super::graduated::flag(name, &mut |key| std::env::var(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_start_measurement_requires_the_durable_reader() {
        let config = RoutingConfig::default();
        assert!(!config.provider_start_measurement_enabled);
        let mut enabled = RoutingConfig {
            provider_start_measurement_enabled: true,
            ..config
        };
        assert!(enabled.validate().is_err());
        enabled.recovery_enabled = true;
        assert!(enabled.validate().is_ok());
    }

    #[test]
    fn self_service_custom_writer_requires_durable_reader() {
        let mut config = RoutingConfig::default();
        assert!(!config.custom_durable_execution_enabled);
        config.custom_durable_execution_enabled = true;
        assert!(config.validate().is_err());
        config.recovery_enabled = true;
        assert!(config.validate().is_ok());
        config.custom_durable_execution_enabled = false;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn replacement_selection_is_explicit_and_can_stop_without_disabling_generation_readers() {
        let mut config = RoutingConfig::default();
        assert!(!config.binding_preparation_enabled);
        assert!(config.binding_replacement_bindings.is_empty());
        assert!(binding_replacements(None).unwrap().is_empty());
        assert!(binding_replacements(Some("*")).is_err());
        assert!(binding_replacements(Some("binding/path")).is_err());
        assert_eq!(
            binding_replacements(Some(" binding_one,binding_one ")).unwrap(),
            BTreeSet::from(["binding_one".into()])
        );
        config
            .binding_replacement_bindings
            .insert("binding_one".into());
        assert!(config.validate().is_err());
        config.recovery_enabled = true;
        config.binding_preparation_enabled = true;
        assert!(config.validate().is_ok());
        config.binding_replacement_bindings.clear();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn admission_requires_recovery_but_recovery_can_continue_after_admission_stops() {
        let mut config = RoutingConfig::default();
        assert!(!config.quoted_context_enabled);
        config.quoted_context_enabled = true;
        assert!(config.validate().is_err());
        config.quoted_context_enabled = false;
        assert!(config.validate().is_ok());
        config.admission_enabled = true;
        assert!(config.validate().is_err());
        config.admission_enabled = false;
        config.execution_records.insert("retained-channel".into());
        assert!(config.validate().is_err());
        config.recovery_enabled = true;
        assert!(config.validate().is_ok());
        config.admission_enabled = true;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn execution_selection_defaults_off_and_rejects_broad_or_invalid_selection() {
        assert!(execution_records(None).unwrap().is_empty());
        assert!(execution_records(Some(" , ")).unwrap().is_empty());
        let record = "11111111-1111-4111-8111-111111111111";
        assert_eq!(
            execution_records(Some(&format!(" {record},{record} "))).unwrap(),
            BTreeSet::from([record.to_owned()])
        );
        assert!(execution_records(Some("*")).is_err());
        assert!(execution_records(Some("invalid/record")).is_err());
        let records = (0..65)
            .map(|n| format!("11111111-1111-4111-8111-{n:012}"))
            .collect::<Vec<_>>();
        assert!(execution_records(Some(&records.join(","))).is_err());
        assert_eq!(
            execution_records(Some(&records[..64].join(",")))
                .unwrap()
                .len(),
            64
        );
    }

    fn registry(slugs: &[&str]) -> ProviderRegistry {
        let entries: Vec<_> = slugs
            .iter()
            .map(|slug| {
                json!({
                    "slug": slug,
                    "base_url": "http://127.0.0.1:8080",
                    "kind": "responses_http",
                    "paths": {
                        "agent_card": "/.well-known/agent-card.json",
                        "binding": "/bindings/{binding_id}",
                        "conversation": "/bindings/{binding_id}/conversations/{conversation_id}",
                        "responses": "/responses"
                    },
                    "max_inflight": 1
                })
            })
            .collect();
        ProviderRegistry::parse(&serde_json::Value::Array(entries).to_string()).unwrap()
    }

    fn parsed(value: Option<&str>, registry: &ProviderRegistry) -> Vec<String> {
        non_human_providers(value, registry)
            .unwrap()
            .into_iter()
            .collect()
    }

    fn detail(error: ConfigError) -> String {
        match error {
            ConfigError::Routing(detail) => detail,
            other => panic!("expected an invalid routing configuration, got {other}"),
        }
    }

    #[test]
    fn unset_selects_cma_codex_when_it_is_configured() {
        assert_eq!(
            parsed(None, &registry(&["cma_codex", "nyxbot"])),
            ["cma_codex"]
        );
    }

    #[test]
    fn unset_selects_nothing_without_cma_codex() {
        assert!(parsed(None, &registry(&["nyxbot"])).is_empty());
        assert!(parsed(None, &ProviderRegistry::default()).is_empty());
    }

    #[test]
    fn an_explicit_list_is_trimmed_and_sorted() {
        let configured = registry(&["cma_codex", "nyxbot"]);
        assert_eq!(
            parsed(Some(" nyxbot , cma_codex "), &configured),
            ["cma_codex", "nyxbot"]
        );
        assert_eq!(parsed(Some("nyxbot"), &configured), ["nyxbot"]);
    }

    #[test]
    fn an_empty_value_names_no_provider() {
        let configured = registry(&["cma_codex"]);
        assert!(parsed(Some(""), &configured).is_empty());
        assert!(parsed(Some(" , ,"), &configured).is_empty());
    }

    #[test]
    fn an_unknown_slug_fails_startup_naming_the_key() {
        let configured = registry(&["cma_codex"]);
        let error = non_human_providers(Some("cma_codex,aevatar"), &configured).unwrap_err();
        assert_eq!(
            error.to_string(),
            "routing configuration is invalid: CMAEG_NON_HUMAN_PROVIDERS names unknown provider aevatar"
        );
        // A configured registry is required even for the default slug.
        assert_eq!(
            detail(
                non_human_providers(Some("cma_codex"), &ProviderRegistry::default()).unwrap_err()
            ),
            "CMAEG_NON_HUMAN_PROVIDERS names unknown provider cma_codex"
        );
        // Not even a slug: it is refused the same way.
        assert_eq!(
            detail(non_human_providers(Some("Not A Slug"), &configured).unwrap_err()),
            "CMAEG_NON_HUMAN_PROVIDERS names unknown provider Not A Slug"
        );
    }

    #[test]
    fn duplicates_collapse() {
        assert_eq!(
            parsed(Some("cma_codex,cma_codex"), &registry(&["cma_codex"])),
            ["cma_codex"]
        );
    }

    #[test]
    fn at_most_eight_distinct_providers_are_accepted() {
        let names: Vec<String> = (0..9).map(|index| format!("provider_{index}")).collect();
        let all: Vec<&str> = names.iter().map(String::as_str).collect();
        let configured = registry(&all);
        assert_eq!(
            detail(non_human_providers(Some(&all.join(",")), &configured).unwrap_err()),
            "CMAEG_NON_HUMAN_PROVIDERS names more than 8 providers"
        );
        let mut eight_with_repeat = all[..8].to_vec();
        eight_with_repeat.push(all[0]);
        assert_eq!(
            parsed(Some(&eight_with_repeat.join(",")), &configured).len(),
            8
        );
    }
}
