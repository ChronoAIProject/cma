//! Channel definitions and their stateless gateway policy.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, str::FromStr};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use utoipa::ToSchema;

use crate::{
    ids::RecordId,
    profile::{EventWorkerProfile, NyxIdIdentity},
    require_string, ValidationError,
};

mod admission;
pub use admission::{AdmissionPolicy, BotMessageAdmission, GroupAdmission, IdentityAdmission};
mod consumer;
mod mutation;
pub use mutation::{ChannelMutation, ChannelMutationKind};
mod custom_attachments;
pub(crate) mod discord;
pub use discord::valid_id as valid_discord_id;
mod extensions;
pub mod lark_resources;
mod lookup;
pub use lark_resources::{LarkMessageEffectsBinding, LarkResourceBinding, LarkResourceKind};
pub mod native_lark;
mod partition;
mod platform;
mod profile_revision;
mod provider;
pub use profile_revision::{
    ProfilePublication, ProfileRevisionView, ProfileRevisions, RetainedProfileRevision,
};
pub use provider::{ConformanceLevel, ProviderBinding, ProviderTarget};
mod routing;
mod routing_validation;
mod secrets;
mod source_id;
mod telegram;
pub mod whatsapp;
pub use consumer::{
    Consumer, ConsumerReply, ConsumerView, DefaultConsumer, EffectiveReply, PublishPolicy,
    ReplyChoice,
};
pub use custom_attachments::{
    CustomAttachments, DEFAULT_CUSTOM_ATTACHMENT_POINTER, MAX_CUSTOM_ATTACHMENT_ORIGINS,
    MAX_CUSTOM_ATTACHMENT_POINTER_BYTES,
};
pub use extensions::Extensions;
pub use native_lark::{
    LarkRegion, NativeLarkCredentials, NativeLarkIdentity, NativeLarkReply, NativeLarkSource,
};
pub use routing::{
    default_receives_all, ActivityKindName, Addressing, AgentEventPolicy, DefaultReply, Pattern,
    ReplyLabel, RoutingPolicy, RoutingRule, RuleMatch, MAX_ADDITIONAL_CONSUMERS, MAX_MATCH_ENTRIES,
    MAX_MATCH_ENTRY_BYTES, MAX_RULES, MAX_TARGETS, ROUTING_POLICY_VERSION,
};
pub use secrets::{ChannelSecrets, Secret};
pub use source_id::{SourceId, AGENT_SOURCE_ID};
pub use whatsapp::WhatsappCredentials;
mod sources;
mod validation;
pub use platform::Platform;
pub(crate) use validation::validate_https_url;
use validation::{validate_admission, validate_hash, validate_relay_identity};

pub const CHANNEL_SCHEMA_VERSION: u16 = 1;
pub const MAX_SOURCES: usize = 4;
/// The most sources a reader accepts: Telegram, one relay per platform and custom.
pub const MAX_EXTENDED_SOURCES: usize = 8;
pub const MAX_ROUTE_IDS: usize = 64;
pub const MAX_SINKS: usize = 3;
pub const MAX_PROMPT_TEMPLATE_BYTES: usize = 8 * 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum Source {
    #[schema(value_type = crate::schema::NativeLarkSource)]
    NativeLark(#[schema(inline)] NativeLarkSource),
    /// Native Meta WhatsApp Cloud API; independent of NyxID relay sources.
    Whatsapp {
        /// Incoming private edits and deletions for WhatsApp Business App coexistence accounts.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        message_events: bool,
        /// Owner consent to create and publish managed static native Flow assets.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        forms_enabled: bool,
        /// Explicit existing group IDs; empty keeps private-only behavior.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schema(max_items = 16)]
        group_ids: Vec<String>,
        app_id: String,
        waba_id: String,
        phone_number_id: String,
        admission: AdmissionPolicy,
    },
    Telegram {
        /// Owner consent to close/reopen only the exact admitted forum topic.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        topic_lifecycle: bool,
        /// Owner consent to create and manage new topics in the original forum supergroup.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        topic_creation: bool,
        /// Retain original message routing for edits and attributable reactions.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        message_events: bool,
        /// Admit native channel posts as non-human channel identities; requires message_events.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        channel_posts: bool,
        /// Exact Telegram groups that opted into one event per human membership update.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schema(max_items = 16)]
        member_join_chat_ids: Vec<String>,
        bot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        admission: AdmissionPolicy,
    },
    /// Native Discord Gateway, independently owned by CMAEG.
    Discord {
        bot_id: String,
        intents: u64,
        /// Allow an admitted consumer to read up to 20 earlier messages in its exact thread.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        thread_history: bool,
        /// Explicit opt-in to bounded native Discord thread lifecycle operations.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        thread_lifecycle: bool,
        /// Explicit read-only pagination within the original admitted conversation.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        conversation_history: bool,
        admission: AdmissionPolicy,
    },
    Github {
        app_ref: String,
        installation_ids: Vec<u64>,
        repository_ids: Vec<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        deny_repository_ids: Vec<u64>,
        events: Vec<String>,
        actions: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_template: Option<String>,
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
        /// Verify Discord/WhatsApp raw facts; omitted on retained normalized sources.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        verify_raw_event: bool,
        /// Optional exact NyxID connection for embedded Lark/Feishu resources.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_resources: Option<LarkResourceBinding>,
        /// Exact Telegram relay groups opted into one event per human membership update.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schema(max_items = 16)]
        member_join_chat_ids: Vec<String>,
        /// Explicit write authority; independent of resource reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_message_effects: Option<Box<LarkMessageEffectsBinding>>,
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
    /// The key for custom events on a chat-keyed channel; absent means `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_key: Option<PartitionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_contract: Option<crate::thread::ThreadContract>,
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
    /// Sealed management custody; never authority supplied by a public request.
    #[serde(default, skip_serializing)]
    pub mutation: Option<ChannelMutation>,
    pub owner: NyxIdIdentity,
    pub profile: EventWorkerProfile,
    pub profile_digest: String,
    pub provider: ProviderBinding,
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    pub partition: PartitionRule,
    pub reply: ReplyPolicy,
    /// Multi-consumer routing; absent on every legacy channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingPolicy>,
    /// Immutable creation opt-in for authenticated Custom durable execution.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub durable_custom_execution: bool,
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
    /// whether restored from storage or admitted by management. It accepts
    /// every legacy definition, the extended source and partition shapes
    /// only where the legacy rules refuse them or `custom_key` is present,
    /// and a routing policy only when it meets its own rules against the
    /// rest of the definition. Writes of those shapes are refused
    /// separately, by the write gate.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(mutation) = &self.mutation {
            mutation.validate()?;
        }
        self.validate_head()?;
        if let Some(revisions) = &self.secrets.profile_revisions {
            revisions.validate(self)?;
        }
        if self.validate_legacy_sources_and_partition().is_err() {
            sources::validate_extended_sources(&self.sources, &self.secrets)?;
            self.partition.validate_extended(&self.sources)?;
        } else if self.partition.custom_key.is_some() {
            self.partition.validate_extended(&self.sources)?;
        }
        self.validate_tail()?;
        if self.durable_custom_execution && self.routing.is_none() {
            return Err(ValidationError::new(
                "/routing",
                "durable Custom execution requires routing",
            ));
        }
        self.validate_routing()
    }

    /// Today's complete validation, kept verbatim; it ignores `routing`,
    /// which older images cannot read at all. The write gate answers a
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
        if let Some(contract) = &self.reply.thread_contract {
            contract.validate(self)?;
        }
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
        // Readers accept a channel with no sources; its replies still need one.
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
}
