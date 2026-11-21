//! Strict provider registry and the only safe URL construction boundary.

use std::{collections::BTreeMap, fmt, net::IpAddr};

use cmaeg_core::ids::ProviderSlug;
use reqwest::Url;
use serde::Deserialize;

use crate::ConfigError;

const REQUIRED_BINDING: &str = "{binding_id}";
const REQUIRED_CONVERSATION: &str = "{conversation_id}";
const REQUIRED_RESPONSE: &str = "{response_id}";

/// The provider protocol implemented by this client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    ResponsesHttp,
}

/// Named provider endpoints. Templates are validated when the registry loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPaths {
    pub agent_card: String,
    pub binding: String,
    pub conversation: String,
    pub responses: String,
    /// Explicit opt-in to CMA Responses v2 snapshot/replay semantics.
    pub recovery: Option<RecoveryPaths>,
    /// Explicit opt-in to management-authenticated generation preparation.
    pub preparation: Option<PreparationPaths>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPaths {
    pub response: String,
    pub events: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationPaths {
    pub inspection: String,
    pub reservation: String,
}

/// An operator-approved provider and its bounded concurrency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    pub slug: ProviderSlug,
    pub base_url: Url,
    pub kind: ProviderKind,
    pub paths: ProviderPaths,
    pub max_inflight: usize,
}

/// Which provider path a request may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPath {
    AgentCard,
    Binding,
    Conversation,
    Responses,
    RecoveryResponse,
    RecoveryEvents,
    PreparationInspection,
    PreparationReservation,
}

impl ProviderPath {
    fn key(self) -> &'static str {
        match self {
            Self::AgentCard => "agent_card",
            Self::Binding => "binding",
            Self::Conversation => "conversation",
            Self::Responses => "responses",
            Self::RecoveryResponse => "recovery_response",
            Self::RecoveryEvents => "recovery_events",
            Self::PreparationInspection => "preparation_inspection",
            Self::PreparationReservation => "preparation_reservation",
        }
    }
}

/// A URL construction refusal, kept separate from transport failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUrlError(String);

impl fmt::Display for ProviderUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProviderUrlError {}

impl ProviderEntry {
    /// Build a URL from a validated path template and encoded path variables.
    pub fn url(
        &self,
        path_key: &str,
        parameters: &[(&str, &str)],
    ) -> Result<Url, ProviderUrlError> {
        let template = match path_key {
            "agent_card" => &self.paths.agent_card,
            "binding" => &self.paths.binding,
            "conversation" => &self.paths.conversation,
            "responses" => &self.paths.responses,
            "recovery_response" | "recovery_events" => {
                let recovery = self.paths.recovery.as_ref().ok_or_else(|| {
                    ProviderUrlError("provider response recovery is not configured".into())
                })?;
                if path_key == "recovery_response" {
                    &recovery.response
                } else {
                    &recovery.events
                }
            }
            "preparation_inspection" | "preparation_reservation" => {
                let preparation = self.paths.preparation.as_ref().ok_or_else(|| {
                    ProviderUrlError("provider binding preparation is not configured".into())
                })?;
                if path_key == "preparation_inspection" {
                    &preparation.inspection
                } else {
                    &preparation.reservation
                }
            }
            other => return Err(ProviderUrlError(format!("unknown provider path {other:?}"))),
        };
        let mut rendered = template.clone();
        let required = required_parameters(path_key);
        if parameters.len() != required.len()
            || parameters.iter().any(|(name, _)| !required.contains(name))
            || required.iter().any(|name| {
                parameters
                    .iter()
                    .filter(|(candidate, _)| candidate == name)
                    .count()
                    != 1
            })
        {
            return Err(ProviderUrlError(format!(
                "provider path {path_key:?} received the wrong path parameters"
            )));
        }
        for (name, value) in parameters {
            if value.is_empty() || matches!(*value, "." | "..") {
                return Err(ProviderUrlError(
                    "path parameter must be a non-empty opaque segment".into(),
                ));
            }
            rendered = rendered.replace(&format!("{{{name}}}"), &encode_segment(value));
        }
        if rendered.contains('{') || rendered.contains('}') {
            return Err(ProviderUrlError(format!(
                "provider path {path_key:?} contains an unresolved placeholder"
            )));
        }
        let url = self
            .base_url
            .join(&rendered)
            .map_err(|_| ProviderUrlError("provider path is not a URL".into()))?;
        if url.origin() != self.base_url.origin() {
            return Err(ProviderUrlError("provider path changed the origin".into()));
        }
        Ok(url)
    }

    /// Typed shorthand for callers that do not need string path keys.
    pub fn typed_url(
        &self,
        path: ProviderPath,
        parameters: &[(&str, &str)],
    ) -> Result<Url, ProviderUrlError> {
        self.url(path.key(), parameters)
    }
}

/// The immutable set of provider entries accepted at process startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderRegistry {
    entries: BTreeMap<ProviderSlug, ProviderEntry>,
}

impl ProviderRegistry {
    /// Parse and validate the JSON operator allowlist.
    pub fn parse(json: &str) -> Result<Self, ConfigError> {
        let raw = serde_json::from_str::<Vec<RawProviderEntry>>(json).map_err(|error| {
            invalid(format!("must be a JSON array of provider entries: {error}"))
        })?;
        let mut entries = BTreeMap::new();
        for candidate in raw {
            let slug = ProviderSlug::new(candidate.slug.clone())
                .map_err(|error| invalid(format!("slug: {error}")))?;
            if !slug
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                || !(1..=32).contains(&slug.as_str().len())
            {
                return Err(invalid(format!(
                    "slug {:?} must match [a-z0-9_]{{1,32}}",
                    slug
                )));
            }
            if entries.contains_key(&slug) {
                return Err(invalid(format!("duplicate provider slug {:?}", slug)));
            }
            let base_url = Url::parse(&candidate.base_url)
                .map_err(|error| invalid(format!("provider {slug}: invalid base_url: {error}")))?;
            validate_base_url(&base_url)
                .map_err(|detail| invalid(format!("provider {slug}: {detail}")))?;
            let paths = ProviderPaths::try_from(candidate.paths)
                .map_err(|detail| invalid(format!("provider {slug}: {detail}")))?;
            if candidate.max_inflight == 0 {
                return Err(invalid(format!(
                    "provider {slug}: max_inflight must be positive"
                )));
            }
            entries.insert(
                slug.clone(),
                ProviderEntry {
                    slug,
                    base_url,
                    kind: candidate.kind,
                    paths,
                    max_inflight: candidate.max_inflight,
                },
            );
        }
        Ok(Self { entries })
    }

    pub fn get(&self, slug: &ProviderSlug) -> Option<&ProviderEntry> {
        self.entries.get(slug)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProviderEntry> {
        self.entries.values()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviderEntry {
    slug: String,
    base_url: String,
    kind: ProviderKind,
    paths: RawProviderPaths,
    max_inflight: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviderPaths {
    agent_card: String,
    binding: String,
    conversation: String,
    responses: String,
    #[serde(default)]
    recovery: Option<RecoveryPaths>,
    #[serde(default)]
    preparation: Option<PreparationPaths>,
}

impl TryFrom<RawProviderPaths> for ProviderPaths {
    type Error = String;

    fn try_from(value: RawProviderPaths) -> Result<Self, Self::Error> {
        validate_path("agent_card", &value.agent_card, &[])?;
        validate_path("binding", &value.binding, &[REQUIRED_BINDING])?;
        validate_path(
            "conversation",
            &value.conversation,
            &[REQUIRED_BINDING, REQUIRED_CONVERSATION],
        )?;
        validate_path("responses", &value.responses, &[])?;
        if let Some(recovery) = &value.recovery {
            validate_path(
                "recovery.response",
                &recovery.response,
                &[REQUIRED_RESPONSE],
            )?;
            validate_path("recovery.events", &recovery.events, &[REQUIRED_RESPONSE])?;
        }
        if let Some(preparation) = &value.preparation {
            validate_path(
                "preparation.inspection",
                &preparation.inspection,
                &[REQUIRED_BINDING],
            )?;
            validate_path(
                "preparation.reservation",
                &preparation.reservation,
                &[REQUIRED_BINDING],
            )?;
            // These operations carry management authority and have one exact
            // CMA path. Generic providers cannot opt in a sibling endpoint.
            if preparation.inspection
                != "/api/v1/cmaeg/bindings/{binding_id}/preparation-inspections"
                || preparation.reservation != "/api/v1/cmaeg/bindings/{binding_id}/preparations"
            {
                return Err("binding preparation requires the exact CMA management paths".into());
            }
        }
        Ok(Self {
            agent_card: value.agent_card,
            binding: value.binding,
            conversation: value.conversation,
            responses: value.responses,
            recovery: value.recovery,
            preparation: value.preparation,
        })
    }
}

fn validate_base_url(url: &Url) -> Result<(), String> {
    if url.cannot_be_a_base()
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "base_url must be an origin URL without credentials, query, or fragment".into(),
        );
    }
    if url.scheme() == "https" {
        if url
            .host()
            .and_then(|host| match host {
                url::Host::Ipv4(address) => Some(IpAddr::V4(address)),
                url::Host::Ipv6(address) => Some(IpAddr::V6(address)),
                url::Host::Domain(_) => None,
            })
            .is_some_and(|ip| is_non_public_ip(&ip))
        {
            return Err("https base_url must not target a private or special-use IP".into());
        }
        return Ok(());
    }
    if url.scheme() != "http" {
        return Err("base_url must use https, or http for loopback/.svc hosts".into());
    }
    let host = url.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback());
    if loopback || host.to_ascii_lowercase().ends_with(".svc") {
        Ok(())
    } else {
        Err("http base_url is allowed only for loopback or .svc hosts".into())
    }
}

fn validate_path(name: &str, path: &str, placeholders: &[&str]) -> Result<(), String> {
    if path.starts_with("//")
        || !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
        || path.contains('%')
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'\\')
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(format!(
            "{name} path must be absolute, normalized, and contain no query or fragment"
        ));
    }
    let mut remainder = path;
    while let Some(open) = remainder.find('{') {
        if remainder[..open].contains('}') {
            return Err(format!("{name} contains an unmatched brace"));
        }
        let after_open = &remainder[open + 1..];
        let Some(close) = after_open.find('}') else {
            return Err(format!("{name} path contains an unterminated placeholder"));
        };
        let token = &after_open[..close];
        let placeholder = format!("{{{token}}}");
        if !placeholders.contains(&placeholder.as_str()) {
            return Err(format!(
                "{name} path contains unsupported placeholder {placeholder}"
            ));
        }
        if path.matches(&placeholder).count() != 1 {
            return Err(format!(
                "{name} path contains duplicate placeholder {placeholder}"
            ));
        }
        remainder = &after_open[close + 1..];
    }
    if remainder.contains('}') {
        return Err(format!(
            "{name} path contains an unmatched placeholder terminator"
        ));
    }
    for placeholder in placeholders {
        if !path.contains(placeholder) {
            return Err(format!("{name} path must contain {placeholder}"));
        }
    }
    Ok(())
}

fn is_non_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51)
                || (octets[0] == 203 && octets[1] == 0)
                || octets[0] >= 240
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            address
                .to_ipv4_mapped()
                .is_some_and(|ip| is_non_public_ip(&IpAddr::V4(ip)))
                || address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || address.is_unicast_link_local()
                || (first & 0xfe00) == 0xfc00
        }
    }
}

fn required_parameters(path_key: &str) -> &'static [&'static str] {
    match path_key {
        "binding" | "preparation_inspection" | "preparation_reservation" => &["binding_id"],
        "conversation" => &["binding_id", "conversation_id"],
        "recovery_response" | "recovery_events" => &["response_id"],
        _ => &[],
    }
}

fn encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
            encoded.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
        }
    }
    encoded
}

fn invalid(detail: String) -> ConfigError {
    ConfigError::InvalidProviders { detail }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
