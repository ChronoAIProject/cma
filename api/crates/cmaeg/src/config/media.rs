//! Inbound media settings: which providers install the gateway's MCP tools.
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

pub const CMAEG_MEDIA_TOOL_PROVIDERS: &str = "CMAEG_MEDIA_TOOL_PROVIDERS";

/// The one provider whose binding contract promises to install the gateway MCP
/// server.
const DEFAULT_TOOL_PROVIDER: &str = "cma_codex";
/// More slugs than any deployment configures providers; a longer list is an
/// operator mistake rather than a policy.
const MAX_TOOL_PROVIDERS: usize = 8;

/// Inbound media settings loaded after, and validated against, the provider
/// registry.
#[derive(Debug, Clone, Default)]
pub struct MediaConfig {
    pub tool_providers: MediaToolProviders,
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
pub(super) fn from_env(registry: &ProviderRegistry) -> Result<MediaToolProviders, ConfigError> {
    match env::var(CMAEG_MEDIA_TOOL_PROVIDERS) {
        Ok(value) => parse(&value, registry),
        Err(env::VarError::NotPresent) => Ok(default_set(registry)),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidMedia {
            detail: format!("{CMAEG_MEDIA_TOOL_PROVIDERS} must be valid UTF-8"),
        }),
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
