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
    Ok(RoutingConfig {
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
    })
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

/// Reader-first authoring gate; an unset value never enables a new stored shape.
pub(super) fn write_flag(name: &str) -> Result<bool, super::ConfigError> {
    match std::env::var(name).as_deref() {
        Ok("true") => Ok(true),
        Ok("false") | Err(std::env::VarError::NotPresent) => Ok(false),
        _ => Err(super::ConfigError::InvalidManagement {
            detail: format!("{name} must be true or false"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
