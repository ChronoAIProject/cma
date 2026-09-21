//! Channel definitions and their stateless gateway policy.

use std::{collections::BTreeSet, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use utoipa::ToSchema;

use crate::{
    ids::{BindingId, ProviderSlug, RecordId},
    profile::{AgentProfileSnapshot, NyxIdIdentity},
    require_string, ValidationError,
};

pub const CHANNEL_SCHEMA_VERSION: u16 = 1;
pub const MAX_SOURCES: usize = 4;
pub const MAX_ROUTE_IDS: usize = 64;
pub const MAX_SINKS: usize = 3;
pub const MAX_PROMPT_TEMPLATE_BYTES: usize = 8 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Telegram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceLevel {
    L1,
    L2,
    L3,
    L4,
    L5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub slug: ProviderSlug,
    pub binding_id: BindingId,
    pub level: ConformanceLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum Source {
    NyxidRelay {
        issuer: String,
        jwks_url: String,
        key_id: String,
        route_ids: Vec<String>,
        platform: Platform,
        admission: AdmissionPolicy,
    },
    Custom {
        auth: CustomAuth,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum CustomAuth {
    Hmac { key_id: String },
    NyxidKey { key_hashes: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum AdmissionPolicy {
    All,
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        senders: IdentityAdmission,
        chats: IdentityAdmission,
        groups: GroupAdmission,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum IdentityAdmission {
    Deny,
    Open,
    Allowlist(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupAdmission {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartitionKey {
    ConversationAndSender,
    Conversation,
    Thread,
    Fixed,
    EventSubject,
    PerEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PartitionRule {
    pub version: u16,
    pub key: PartitionKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyPolicy {
    pub auto_final: bool,
    pub ack_after_ms: u32,
    #[serde(default)]
    pub sinks: Vec<Sink>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum Sink {
    Source,
    ResultWebhook { url: String, hmac_key_id: String },
    Sync,
}

/// A secret that only appears in the encrypted channel blob.
#[derive(Clone, PartialEq, Eq, ToSchema)]
#[schema(value_type = String)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(String::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelSecrets {
    pub agent_key_hash: String,
    #[serde(default, skip_serializing)]
    pub channel_key: Option<Secret>,
    #[serde(default, skip_serializing)]
    pub hmac_secrets: Vec<(String, Secret)>,
    #[serde(default, skip_serializing)]
    pub provider_credential: Option<Secret>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelDefinition {
    pub schema_version: u16,
    pub record_id: RecordId,
    pub owner: NyxIdIdentity,
    pub profile: AgentProfileSnapshot,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    pub sources: Vec<Source>,
    pub partition: PartitionRule,
    pub reply: ReplyPolicy,
    #[serde(default, skip_serializing)]
    pub secrets: ChannelSecrets,
    pub created_at: String,
    pub version: u64,
}

impl ChannelDefinition {
    /// Validate the complete definition before it is persisted.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != CHANNEL_SCHEMA_VERSION {
            return Err(ValidationError::new(
                "/schema_version",
                "unsupported schema version",
            ));
        }
        self.profile.validate().map_err(|error| {
            ValidationError::new(format!("/profile{}", error.pointer), error.message)
        })?;
        if self.profile_digest != self.profile.digest() {
            return Err(ValidationError::new(
                "/profile_digest",
                "does not match profile",
            ));
        }
        if self.sources.is_empty() || self.sources.len() > MAX_SOURCES {
            return Err(ValidationError::new(
                "/sources",
                "must contain one to four sources",
            ));
        }
        let mut variants = BTreeSet::new();
        let mut relay_platforms = BTreeSet::new();
        let mut has_custom = false;
        for (index, source) in self.sources.iter().enumerate() {
            let pointer = format!("/sources/{index}");
            match source {
                Source::NyxidRelay {
                    issuer,
                    jwks_url,
                    key_id,
                    route_ids,
                    platform,
                    admission,
                } => {
                    if !variants.insert("nyxid_relay") {
                        return Err(ValidationError::new(
                            &pointer,
                            "source variant is duplicated",
                        ));
                    }
                    if !relay_platforms.insert(*platform) {
                        return Err(ValidationError::new(&pointer, "platform is duplicated"));
                    }
                    require_string(issuer, &format!("{pointer}/issuer"), 1, 512)?;
                    require_string(key_id, &format!("{pointer}/key_id"), 1, 512)?;
                    validate_https_url(jwks_url, &format!("{pointer}/jwks_url"))?;
                    validate_nonempty_unique(
                        route_ids,
                        &format!("{pointer}/route_ids"),
                        MAX_ROUTE_IDS,
                    )?;
                    validate_admission(admission, &pointer)?;
                }
                Source::Custom {
                    auth,
                    prompt_template,
                    schema,
                } => {
                    if has_custom {
                        return Err(ValidationError::new(
                            &pointer,
                            "source variant is duplicated",
                        ));
                    }
                    has_custom = true;
                    variants.insert("custom");
                    validate_custom_auth(auth, &pointer)?;
                    if let Some(prompt) = prompt_template {
                        require_string(
                            prompt,
                            &format!("{pointer}/prompt_template"),
                            0,
                            MAX_PROMPT_TEMPLATE_BYTES,
                        )?;
                    }
                    if schema.is_some() {
                        return Err(ValidationError::new(
                            format!("{pointer}/schema"),
                            "must be null in v1",
                        ));
                    }
                }
            }
        }
        self.partition.validate(&self.sources)?;
        if self.reply.sinks.len() > MAX_SINKS {
            return Err(ValidationError::new("/reply/sinks", "too many sinks"));
        }
        for (index, sink) in self.reply.sinks.iter().enumerate() {
            if let Sink::ResultWebhook { url, hmac_key_id } = sink {
                validate_https_url(url, &format!("/reply/sinks/{index}/url"))?;
                require_string(
                    hmac_key_id,
                    &format!("/reply/sinks/{index}/hmac_key_id"),
                    1,
                    512,
                )?;
            }
        }
        if self
            .reply
            .sinks
            .iter()
            .any(|sink| matches!(sink, Sink::Source))
            && self.sources.is_empty()
        {
            return Err(ValidationError::new(
                "/reply/sinks",
                "source sink needs a source",
            ));
        }
        validate_hash(&self.secrets.agent_key_hash, "/secrets/agent_key_hash")?;
        require_string(&self.owner.subject, "/owner/subject", 1, 512)?;
        OffsetDateTime::parse(&self.created_at, &Rfc3339)
            .map_err(|_| ValidationError::new("/created_at", "must be RFC 3339"))?;
        Ok(())
    }

    /// Serialize the private blob consumed by the sealing layer.
    pub fn to_blob_json(&self) -> Value {
        let mut object = serde_json::to_value(self)
            .expect("channel definition is serializable without private fields")
            .as_object()
            .cloned()
            .expect("channel definition is an object");
        object.insert("secrets".into(), json!({
            "agent_key_hash": self.secrets.agent_key_hash,
            "channel_key": self.secrets.channel_key.as_ref().map(Secret::as_str),
            "hmac_secrets": self.secrets.hmac_secrets.iter().map(|(key, secret)| {
                json!([key, secret.as_str()])
            }).collect::<Vec<_>>(),
            "provider_credential": self.secrets.provider_credential.as_ref().map(Secret::as_str),
        }));
        Value::Object(object)
    }

    /// Serialize the public view, retaining only the hash needed for auth.
    pub fn to_public_json(&self) -> Value {
        let mut object = serde_json::to_value(self)
            .expect("channel definition is serializable without private fields")
            .as_object()
            .cloned()
            .expect("channel definition is an object");
        object.insert(
            "secrets".into(),
            json!({ "agent_key_hash": self.secrets.agent_key_hash }),
        );
        Value::Object(object)
    }
}

impl PartitionRule {
    fn validate(&self, sources: &[Source]) -> Result<(), ValidationError> {
        if self.version != 1 {
            return Err(ValidationError::new(
                "/partition/version",
                "unsupported version",
            ));
        }
        let has_custom = sources
            .iter()
            .any(|source| matches!(source, Source::Custom { .. }));
        let has_relay = sources
            .iter()
            .any(|source| matches!(source, Source::NyxidRelay { .. }));
        if matches!(
            self.key,
            PartitionKey::EventSubject | PartitionKey::PerEvent
        ) && !has_custom
        {
            return Err(ValidationError::new(
                "/partition/key",
                "requires a custom source",
            ));
        }
        if matches!(self.key, PartitionKey::Thread) && !has_relay {
            return Err(ValidationError::new(
                "/partition/key",
                "requires a NyxID relay source",
            ));
        }
        Ok(())
    }
}

fn validate_admission(admission: &AdmissionPolicy, pointer: &str) -> Result<(), ValidationError> {
    let values = match admission {
        AdmissionPolicy::Allowlist { sender_ids } => Some(sender_ids),
        AdmissionPolicy::Scoped { senders, chats, .. } => {
            for (name, value) in [("senders", senders), ("chats", chats)] {
                if let IdentityAdmission::Allowlist(values) = value {
                    validate_nonempty_unique(values, &format!("{pointer}/admission/{name}"), 512)?;
                }
            }
            None
        }
        AdmissionPolicy::All => None,
    };
    if let Some(values) = values {
        validate_nonempty_unique(values, &format!("{pointer}/admission"), 512)?;
    }
    Ok(())
}

fn validate_custom_auth(auth: &CustomAuth, pointer: &str) -> Result<(), ValidationError> {
    match auth {
        CustomAuth::Hmac { key_id } => {
            require_string(key_id, &format!("{pointer}/auth/key_id"), 1, 512)
        }
        CustomAuth::NyxidKey { key_hashes } => {
            validate_nonempty_unique(key_hashes, &format!("{pointer}/auth/key_hashes"), 512)?;
            for hash in key_hashes {
                validate_hash(hash, &format!("{pointer}/auth/key_hashes"))?;
            }
            Ok(())
        }
    }
}

fn validate_nonempty_unique(
    values: &[String],
    pointer: &str,
    max: usize,
) -> Result<(), ValidationError> {
    if values.is_empty() || values.len() > max {
        return Err(ValidationError::new(
            pointer,
            "must contain a bounded non-empty list",
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        require_string(value, &format!("{pointer}/{index}"), 1, 512)?;
        if !seen.insert(value) {
            return Err(ValidationError::new(pointer, "values must be unique"));
        }
    }
    Ok(())
}

fn validate_hash(value: &str, pointer: &str) -> Result<(), ValidationError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ValidationError::new(
            pointer,
            "must be sha256: followed by lowercase hex",
        ));
    }
    Ok(())
}

pub(crate) fn validate_https_url(value: &str, pointer: &str) -> Result<(), ValidationError> {
    let (scheme, rest) = value
        .split_once("://")
        .ok_or_else(|| ValidationError::new(pointer, "must be an absolute URL"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();
    if authority.is_empty() || host.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(
            pointer,
            "must include an absolute host",
        ));
    }
    if scheme == "https"
        || (scheme == "http"
            && matches!(
                host.trim_matches(['[', ']']),
                "127.0.0.1" | "localhost" | "::1"
            ))
    {
        Ok(())
    } else {
        Err(ValidationError::new(
            pointer,
            "must use https (or local http)",
        ))
    }
}
impl FromStr for Platform {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "telegram" => Ok(Self::Telegram),
            _ => Err(ValidationError::new("/platform", "unsupported platform")),
        }
    }
}
