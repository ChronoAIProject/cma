//! The channel definition shape an older image reads, frozen.
//!
//! These types copy the serde structure of `ChannelDefinition`, `Source`,
//! `PartitionRule`, `ReplyPolicy`, `Sink`, `ProviderBinding` and
//! `ChannelSecrets` exactly as the image preceding the multi-consumer
//! changes declared them, `deny_unknown_fields` included. A blob that parses
//! here can be read by that image during a rollout or after a rollback; one
//! that does not would make the older image refuse the channel.
//!
//! This file is a compatibility contract, not a model of the current code.
//! It must never be edited to make a test pass: a new stored field belongs in
//! the current types only, and a blob that stops parsing here is exactly the
//! regression these tests exist to catch. Leaf types that the extensions
//! leave untouched are reused from `cmaeg_core`.
use cmaeg_core::{
    channel::{
        AdmissionPolicy, ChannelCreation, ConformanceLevel, CustomAttachments, CustomAuth,
        PartitionKey, Platform, ProviderTarget,
    },
    profile::{EventWorkerProfile, NyxIdIdentity},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The stored definition, including the private `secrets` object of the blob.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelDefinition {
    schema_version: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    record_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation: Option<ChannelCreation>,
    owner: NyxIdIdentity,
    profile: EventWorkerProfile,
    profile_digest: String,
    provider: ProviderBinding,
    sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connection_ref: Option<String>,
    partition: PartitionRule,
    reply: ReplyPolicy,
    secrets: ChannelSecrets,
    created_at: String,
    version: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderBinding {
    slug: String,
    #[serde(default, skip_serializing_if = "ProviderTarget::is_profile")]
    target: ProviderTarget,
    binding_id: String,
    level: ConformanceLevel,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
enum Source {
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<CustomAttachments>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartitionRule {
    version: u16,
    key: PartitionKey,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyPolicy {
    auto_final: bool,
    ack_after_ms: u32,
    #[serde(default)]
    sinks: Vec<Sink>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
enum Sink {
    Source {},
    ResultWebhook { url: String, hmac_key_id: String },
    Sync {},
}

/// The blob's secrets object, which the sealing layer writes with explicit
/// nulls and `[key_id, secret]` pairs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelSecrets {
    agent_key_hash: String,
    #[serde(default)]
    channel_key: Option<String>,
    #[serde(default)]
    hmac_secrets: Vec<(String, String)>,
    #[serde(default)]
    provider_credential: Option<String>,
    #[serde(default)]
    telegram_bot_token: Option<String>,
    #[serde(default)]
    telegram_webhook_secret: Option<String>,
}
