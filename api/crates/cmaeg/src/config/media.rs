//! Inbound media settings: the master switch, which providers install the
//! gateway's MCP tools, and the byte, job and rate budgets every media fetch
//! runs under.
//!
//! Neither the agent card's conformance level nor the binding request proves
//! that a provider installed the gateway MCP server, so whether an agent may be
//! told about gateway tools is operator configuration. It is a separate key
//! rather than a field of a `CMAEG_PROVIDERS` entry because those entries are
//! `deny_unknown_fields`: an older image reading a newer ConfigMap would refuse
//! to start.
use super::ConfigError;
use crate::providers::ProviderRegistry;
use cmaeg_core::ids::ProviderSlug;
use std::{collections::BTreeSet, env};

mod budgets;
pub(super) use budgets::bounded;

pub const CMAEG_MEDIA_TOOL_PROVIDERS: &str = "CMAEG_MEDIA_TOOL_PROVIDERS";
pub const CMAEG_MEDIA_ENABLED: &str = "CMAEG_MEDIA_ENABLED";
pub const CMAEG_MEDIA_MAX_BYTES: &str = "CMAEG_MEDIA_MAX_BYTES";
pub const CMAEG_MEDIA_INFLIGHT_BYTES: &str = "CMAEG_MEDIA_INFLIGHT_BYTES";
pub const CMAEG_MEDIA_RESPONSE_BYTES: &str = "CMAEG_MEDIA_RESPONSE_BYTES";
pub const CMAEG_MEDIA_MAX_JOBS: &str = "CMAEG_MEDIA_MAX_JOBS";
pub const CMAEG_MEDIA_CACHE_BYTES: &str = "CMAEG_MEDIA_CACHE_BYTES";
pub const CMAEG_MEDIA_RATE_PER_MINUTE: &str = "CMAEG_MEDIA_RATE_PER_MINUTE";
pub const CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR: &str = "CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR";

const MIB: u64 = 1024 * 1024;
/// The one provider whose binding contract promises to install the gateway MCP
/// server.
const DEFAULT_TOOL_PROVIDER: &str = "cma_codex";
/// More slugs than any deployment configures providers; a longer list is an
/// operator mistake rather than a policy.
const MAX_TOOL_PROVIDERS: usize = 8;

/// Inbound media settings loaded after, and validated against, the provider
/// registry.
///
/// The budgets are validated at startup even while media is off, so turning
/// the switch on can never be the first moment a bad value is noticed.
#[derive(Debug, Clone)]
pub struct MediaConfig {
    pub tool_providers: MediaToolProviders,
    /// Master switch for inbound media. Off, every media message is refused
    /// as `attachment_unsupported` exactly as before media existed.
    pub enabled: bool,
    /// Bytes fetched for one attachment.
    pub max_bytes: u64,
    /// Pod-wide bytes held by fetched originals and transform outputs.
    pub inflight_bytes: u64,
    /// Pod-wide weighted bytes of media response bodies in flight.
    pub response_bytes: u64,
    /// Pod-wide concurrent media jobs.
    pub max_jobs: u32,
    /// Derived-media cache capacity; zero disables caching.
    pub cache_bytes: u64,
    /// Fetch starts per channel per minute.
    pub rate_per_minute: u32,
    /// Download bytes per channel per hour.
    pub download_bytes_per_hour: u64,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            tool_providers: MediaToolProviders::default(),
            enabled: false,
            max_bytes: 20 * MIB,
            inflight_bytes: 64 * MIB,
            response_bytes: 24 * MIB,
            max_jobs: 6,
            cache_bytes: 32 * MIB,
            rate_per_minute: 60,
            download_bytes_per_hour: 256 * MIB,
        }
    }
}

/// Read every media setting; the tool-provider set is checked against the
/// registry, so this can only run after the registry is loaded.
pub(super) fn config_from_env(registry: &ProviderRegistry) -> Result<MediaConfig, ConfigError> {
    let tool_providers = from_env(registry)?;
    let enabled = enabled_from_env()?;
    budgets::budgets(read_env).map(|config| MediaConfig {
        tool_providers,
        enabled,
        ..config
    })
}

/// One media key from the process environment: unset is `None`, and a value
/// that is not UTF-8 fails startup naming the key.
pub(super) fn read_env(key: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidMedia {
            detail: format!("{key} must be valid UTF-8"),
        }),
    }
}

/// Providers whose profile bindings install the gateway's MCP tools. Only
/// their agents are told how to open an attachment through a gateway tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaToolProviders(BTreeSet<ProviderSlug>);

impl MediaToolProviders {
    pub fn contains(&self, slug: &ProviderSlug) -> bool {
        self.0.contains(slug)
    }
}

impl FromIterator<ProviderSlug> for MediaToolProviders {
    fn from_iter<I: IntoIterator<Item = ProviderSlug>>(slugs: I) -> Self {
        Self(slugs.into_iter().collect())
    }
}

/// Read the tool-provider set.
///
/// Unset selects the default only when the registry configures it, so a
/// gateway without that provider still starts. A set value, including the
/// empty string, is strict: every slug must name a configured provider.
fn from_env(registry: &ProviderRegistry) -> Result<MediaToolProviders, ConfigError> {
    match env::var(CMAEG_MEDIA_TOOL_PROVIDERS) {
        Ok(value) => parse(&value, registry),
        Err(env::VarError::NotPresent) => Ok(default_set(registry)),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidMedia {
            detail: format!("{CMAEG_MEDIA_TOOL_PROVIDERS} must be valid UTF-8"),
        }),
    }
}

/// Read the master switch. Unset is off; a set value must be exactly `true` or
/// `false`, so a typo such as `True` or `1` fails startup instead of silently
/// leaving media off.
fn enabled_from_env() -> Result<bool, ConfigError> {
    match env::var(CMAEG_MEDIA_ENABLED) {
        Ok(value) => parse_enabled(Some(&value)),
        Err(env::VarError::NotPresent) => parse_enabled(None),
        Err(env::VarError::NotUnicode(_)) => Err(invalid_switch()),
    }
}

fn parse_enabled(value: Option<&str>) -> Result<bool, ConfigError> {
    let enabled = match value {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(invalid_switch()),
    };
    tracing::info!(key = CMAEG_MEDIA_ENABLED, enabled, "inbound media switch");
    Ok(enabled)
}

fn invalid_switch() -> ConfigError {
    ConfigError::InvalidMedia {
        detail: format!("{CMAEG_MEDIA_ENABLED} must be true or false"),
    }
}

fn default_set(registry: &ProviderRegistry) -> MediaToolProviders {
    match ProviderSlug::new(DEFAULT_TOOL_PROVIDER) {
        Ok(slug) if registry.get(&slug).is_some() => MediaToolProviders::from_iter([slug]),
        _ => {
            tracing::info!(
                key = CMAEG_MEDIA_TOOL_PROVIDERS,
                "default media tool provider cma_codex is not configured; no tool hints will be rendered"
            );
            MediaToolProviders::default()
        }
    }
}

fn parse(value: &str, registry: &ProviderRegistry) -> Result<MediaToolProviders, ConfigError> {
    let mut slugs = BTreeSet::new();
    for entry in value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let slug = ProviderSlug::new(entry)
            .ok()
            .filter(|slug| registry.get(slug).is_some())
            .ok_or_else(|| ConfigError::InvalidMedia {
                detail: format!("{CMAEG_MEDIA_TOOL_PROVIDERS} names unknown provider {entry}"),
            })?;
        slugs.insert(slug);
        if slugs.len() > MAX_TOOL_PROVIDERS {
            return Err(ConfigError::InvalidMedia {
                detail: format!(
                    "{CMAEG_MEDIA_TOOL_PROVIDERS} names more than {MAX_TOOL_PROVIDERS} providers"
                ),
            });
        }
    }
    tracing::debug!(
        key = CMAEG_MEDIA_TOOL_PROVIDERS,
        providers = slugs.len(),
        "media tool providers configured"
    );
    Ok(MediaToolProviders(slugs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    fn slugs(set: &MediaToolProviders) -> Vec<&str> {
        set.0.iter().map(ProviderSlug::as_str).collect()
    }

    fn detail(error: ConfigError) -> String {
        match error {
            ConfigError::InvalidMedia { detail } => detail,
            other => panic!("expected an invalid media configuration, got {other}"),
        }
    }

    #[test]
    fn the_media_switch_is_off_unless_exactly_true() {
        assert!(!parse_enabled(None).unwrap());
        assert!(!parse_enabled(Some("false")).unwrap());
        assert!(parse_enabled(Some("true")).unwrap());
        for value in ["", "True", "TRUE", "1", "yes", " true", "true ", "on"] {
            assert_eq!(
                detail(parse_enabled(Some(value)).unwrap_err()),
                "CMAEG_MEDIA_ENABLED must be true or false",
                "{value:?}"
            );
        }
    }

    #[test]
    fn unset_selects_cma_codex_only_when_it_is_configured() {
        let configured = registry(&["cma_codex", "nyxbot"]);
        let set = default_set(&configured);
        assert_eq!(slugs(&set), ["cma_codex"]);
        assert!(set.contains(&"cma_codex".parse().unwrap()));
        assert!(!set.contains(&"nyxbot".parse().unwrap()));
        assert_eq!(
            default_set(&ProviderRegistry::default()),
            MediaToolProviders::default()
        );
        assert!(default_set(&registry(&["nyxbot"])).0.is_empty());
    }

    #[test]
    fn set_values_are_trimmed_and_empty_entries_ignored() {
        let configured = registry(&["cma_codex", "nyxbot"]);
        assert!(parse("", &configured).unwrap().0.is_empty());
        assert!(parse(" , ,", &configured).unwrap().0.is_empty());
        assert_eq!(
            slugs(&parse(" cma_codex , ", &configured).unwrap()),
            ["cma_codex"]
        );
        assert_eq!(
            slugs(&parse("nyxbot,cma_codex", &configured).unwrap()),
            ["cma_codex", "nyxbot"]
        );
    }

    #[test]
    fn unknown_providers_are_refused_by_name() {
        let configured = registry(&["cma_codex", "nyxbot"]);
        let error = parse("nyxbot,unknown", &configured).unwrap_err();
        assert_eq!(
            error.to_string(),
            "media configuration is invalid: CMAEG_MEDIA_TOOL_PROVIDERS names unknown provider unknown"
        );
        assert_eq!(
            detail(parse("cma_codex", &ProviderRegistry::default()).unwrap_err()),
            "CMAEG_MEDIA_TOOL_PROVIDERS names unknown provider cma_codex"
        );
        // Longer than any identifier: it cannot even form a slug.
        let oversized = "a".repeat(600);
        assert_eq!(
            detail(parse(&oversized, &configured).unwrap_err()),
            format!("CMAEG_MEDIA_TOOL_PROVIDERS names unknown provider {oversized}")
        );
    }

    #[test]
    fn at_most_eight_distinct_providers_are_accepted() {
        let names: Vec<String> = (0..9).map(|index| format!("provider_{index}")).collect();
        let all: Vec<&str> = names.iter().map(String::as_str).collect();
        let configured = registry(&all);
        assert_eq!(
            detail(parse(&all.join(","), &configured).unwrap_err()),
            "CMAEG_MEDIA_TOOL_PROVIDERS names more than 8 providers"
        );
        let mut eight_with_repeat = all[..8].to_vec();
        eight_with_repeat.push(all[0]);
        assert_eq!(
            parse(&eight_with_repeat.join(","), &configured)
                .unwrap()
                .0
                .len(),
            8
        );
        assert_eq!(
            slugs(&parse("cma_codex,cma_codex", &registry(&["cma_codex"])).unwrap()),
            ["cma_codex"]
        );
    }
}
