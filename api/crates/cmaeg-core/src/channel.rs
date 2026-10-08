//! Channel definitions and their stateless gateway policy.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, str::FromStr};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use utoipa::ToSchema;

use crate::{
    ids::{BindingId, ProviderSlug, RecordId},
    profile::{EventWorkerProfile, NyxIdIdentity},
    require_string, ValidationError,
};

mod consumer;
mod custom_attachments;
mod partition;
mod platform;
mod secrets;
mod source_id;
mod telegram;
pub use consumer::ConsumerView;
pub use custom_attachments::{
    CustomAttachments, DEFAULT_CUSTOM_ATTACHMENT_POINTER, MAX_CUSTOM_ATTACHMENT_ORIGINS,
    MAX_CUSTOM_ATTACHMENT_POINTER_BYTES,
};
pub use secrets::{ChannelSecrets, Secret};
pub use source_id::{SourceId, AGENT_SOURCE_ID};
mod sources;
mod validation;
pub use platform::Platform;
pub(crate) use validation::validate_https_url;
use validation::{validate_admission, validate_hash, validate_relay_identity};

pub const CHANNEL_SCHEMA_VERSION: u16 = 1;
pub const MAX_SOURCES: usize = 4;
pub const MAX_ROUTE_IDS: usize = 64;
pub const MAX_SINKS: usize = 3;
pub const MAX_PROMPT_TEMPLATE_BYTES: usize = 8 * 1_024;

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
    #[serde(default, skip_serializing_if = "ProviderTarget::is_profile")]
    pub target: ProviderTarget,
    pub binding_id: BindingId,
    pub level: ConformanceLevel,
}

/// Generic provider target. Instance selection and authorization stay with the
/// provider; the gateway only retains and forwards this declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum ProviderTarget {
    Profile {},
    ExistingAgent { agent_id: String },
}

impl Default for ProviderTarget {
    fn default() -> Self {
        Self::Profile {}
    }
}

impl ProviderTarget {
    pub fn is_profile(&self) -> bool {
        matches!(self, Self::Profile {})
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Self::ExistingAgent { agent_id } = self {
            require_string(agent_id, "/provider/target/agent_id", 1, 128)?;
            if !agent_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err(ValidationError::new(
                    "/provider/target/agent_id",
                    "must be a provider agent identifier",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum Source {
    Telegram {
        bot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        admission: AdmissionPolicy,
    },
    NyxidRelay {
        issuer: String,
        jwks_url: String,
        key_id: String,
        route_ids: Vec<String>,
        platform: Platform,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        // The bot's own platform user ID, such as a Lark bot `open_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
        // Explicit opt-in to admit a platform without a raw-event verifier.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        trust_normalized: bool,
        admission: AdmissionPolicy,
    },
    Custom {
        auth: CustomAuth,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Value>,
        // Omitted when absent, so definitions without it keep their bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<CustomAttachments>,
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
    All {},
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
#[serde(
    rename_all = "snake_case",
    tag = "type",
    content = "ids",
    deny_unknown_fields
)]
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
    Source {},
    ResultWebhook { url: String, hmac_key_id: String },
    Sync {},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelDefinition {
    pub schema_version: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub record_id: RecordId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation: Option<ChannelCreation>,
    pub owner: NyxIdIdentity,
    pub profile: EventWorkerProfile,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    pub partition: PartitionRule,
    pub reply: ReplyPolicy,
    #[serde(default, skip_serializing)]
    pub secrets: ChannelSecrets,
    pub created_at: String,
    pub version: u64,
}

/// Retained management admission, independent of process-local idempotency caches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreation {
    pub channel_id: crate::ids::ChannelId,
    pub fingerprint: String,
    pub ready: bool,
}

impl ChannelDefinition {
    /// The reader: the rules every definition this image accepts must meet,
    /// whether restored from storage or admitted by management. It applies
    /// exactly the legacy rules until readers that understand extended
    /// routing widen it.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.validate_legacy()
    }

    /// Today's complete validation, kept verbatim. The write gate answers a
    /// write that older images could not read with this error while its flag
    /// is off, so the caller sees the pointer and message it always has.
    pub fn validate_legacy(&self) -> Result<(), ValidationError> {
        // The first error wins, so this order is part of the refusal contract.
        self.validate_head()?;
        self.validate_legacy_sources_and_partition()?;
        self.validate_tail()
    }

    /// Today's source and partition rules alone. They are the only rules the
    /// multi-consumer extensions relax, so they decide whether an older image
    /// can still read a definition, for readers and the routing audit alike.
    pub fn validate_legacy_sources_and_partition(&self) -> Result<(), ValidationError> {
        sources::validate_legacy_sources(&self.sources, &self.secrets)?;
        self.partition.validate_legacy(&self.sources)
    }

    fn validate_head(&self) -> Result<(), ValidationError> {
        if self.schema_version != CHANNEL_SCHEMA_VERSION {
            return Err(ValidationError::new(
                "/schema_version",
                "unsupported schema version",
            ));
        }
        require_string(&self.name, "/name", 0, 80)?;
        if let Some(reference) = &self.connection_ref {
            require_string(reference, "/connection_ref", 1, 4096)?;
        }
        self.provider.target.validate()?;
        self.profile.validate().map_err(|error| {
            ValidationError::new(format!("/profile{}", error.pointer), error.message)
        })?;
        if self.profile_digest != self.profile.digest() {
            return Err(ValidationError::new(
                "/profile_digest",
                "does not match profile",
            ));
        }
        Ok(())
    }

    fn validate_tail(&self) -> Result<(), ValidationError> {
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
            .any(|sink| matches!(sink, Sink::Source {}))
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
            "telegram_bot_token": self.secrets.telegram_bot_token.as_ref().map(Secret::as_str),
            "telegram_webhook_secret": self.secrets.telegram_webhook_secret.as_ref().map(Secret::as_str),
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
