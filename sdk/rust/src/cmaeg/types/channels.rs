//! Channels wire types for the independent CMAEG v1 protocol.
use super::{
    BindingId, ChannelId, ConformanceLevel, EventWorkerProfile, GroupAdmission, NyxIdIdentity,
    PartitionKey, Platform, ProviderSlug, RecordId, Secret,
};
use serde::{Deserialize, Serialize};

/// CMAEG `AdmissionPolicy` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdmissionPolicy {
    /// Wire variant `all`.
    #[serde(rename = "all")]
    All {},
    /// Wire variant `allowlist`.
    #[serde(rename = "allowlist")]
    Allowlist {
        /// Wire field `sender_ids`.
        sender_ids: Vec<String>,
    },
    /// Wire variant `scoped`.
    #[serde(rename = "scoped")]
    Scoped {
        /// Wire field `chats`.
        chats: IdentityAdmission,
        /// Wire field `groups`.
        groups: GroupAdmission,
        /// Wire field `senders`.
        senders: IdentityAdmission,
    },
}

/// CMAEG `ChannelCreateRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelCreateRequest {
    /// Opaque gateway connection selected for this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    /// Admission policy applied to the selected connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_admission: Option<AdmissionPolicy>,
    /// Wire field `credentials`.
    pub credentials: CredentialsRequest,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `partition`.
    pub partition: PartitionRule,
    /// Wire field `profile`.
    pub profile: EventWorkerProfile,
    /// Wire field `provider`.
    pub provider: ProviderRequest,
    /// Wire field `record_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<RecordId>,
    /// Wire field `reply`.
    pub reply: ReplyPolicy,
    /// Wire field `schema_version`.
    pub schema_version: u64,
    /// Wire field `sources`.
    pub sources: Vec<SourceRequest>,
}

/// CMAEG `ChannelCreation` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelCreation {
    /// Wire field `channel_id`.
    pub channel_id: ChannelId,
    /// Wire field `fingerprint`.
    pub fingerprint: String,
    /// Wire field `ready`.
    pub ready: bool,
}

/// CMAEG `ChannelResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelResponse {
    /// Wire field `channel_id`.
    pub channel_id: String,
    /// Wire field `definition`.
    pub definition: PublicChannelDefinition,
    /// Wire field `endpoints`.
    pub endpoints: EndpointResponse,
    /// Wire field `provider`.
    pub provider: ProviderResponse,
    /// Wire field `record_id`.
    pub record_id: String,
    /// Wire field `version`.
    pub version: u64,
}

/// CMAEG owner-scoped channel listing with an opaque continuation cursor.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelPage {
    /// Channels visible to the authenticated creator.
    pub items: Vec<ChannelResponse>,
    /// Cursor for the next page, when more channels remain.
    pub next_cursor: Option<String>,
}

/// CMAEG `ChannelUpdateRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelUpdateRequest {
    /// Opaque gateway connection selected for this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    /// Admission policy applied to the selected connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_admission: Option<AdmissionPolicy>,
    /// Wire field `credentials`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<CredentialPatch>,
    /// Wire field `expected_version`.
    pub expected_version: u64,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `partition`.
    pub partition: PartitionRule,
    /// Wire field `profile`.
    pub profile: EventWorkerProfile,
    /// Wire field `provider`.
    pub provider: ProviderRequest,
    /// Wire field `reply`.
    pub reply: ReplyPolicy,
    /// Wire field `schema_version`.
    pub schema_version: u64,
    /// Wire field `sources`.
    pub sources: Vec<SourceRequest>,
}

/// CMAEG `CredentialPatch` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialPatch {
    /// Wire field `agent_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_key: Option<Secret>,
    /// Wire field `channel_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_key: Option<Secret>,
    /// Native Telegram bot credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_token: Option<Secret>,
    /// Native Telegram webhook authentication secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_webhook_secret: Option<Secret>,
    /// Wire field `hmac_secrets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac_secrets: Option<Vec<HmacSecretRequest>>,
}

/// CMAEG `CredentialsRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CredentialsRequest {
    /// Wire field `agent_key`.
    pub agent_key: Secret,
    /// Wire field `channel_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_key: Option<Secret>,
    /// Native Telegram bot credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_token: Option<Secret>,
    /// Native Telegram webhook authentication secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_webhook_secret: Option<Secret>,
    /// Wire field `hmac_secrets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac_secrets: Option<Vec<HmacSecretRequest>>,
}

/// CMAEG `CustomAuth` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CustomAuth {
    /// Wire variant `hmac`.
    #[serde(rename = "hmac")]
    Hmac {
        /// Wire field `key_id`.
        key_id: String,
    },
    /// Wire variant `nyxid_key`.
    #[serde(rename = "nyxid_key")]
    NyxidKey {
        /// Wire field `key_hashes`.
        key_hashes: Vec<String>,
    },
}

/// CMAEG `EndpointResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EndpointResponse {
    /// Wire field `custom_event_url`.
    pub custom_event_url: String,
    /// Wire field `nyxid_callback_url`.
    pub nyxid_callback_url: String,
    /// Native Telegram callback endpoint, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_webhook_url: Option<String>,
}

/// CMAEG `FieldRefusal` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRefusal {
    /// Wire field `field`.
    pub field: String,
    /// Wire field `reason`.
    pub reason: String,
}

/// CMAEG `HmacSecretRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct HmacSecretRequest {
    /// Wire field `key_id`.
    pub key_id: String,
    /// Wire field `secret`.
    pub secret: Secret,
}

/// CMAEG `IdentityAdmission` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IdentityAdmission {
    /// Wire variant `deny`.
    #[serde(rename = "deny")]
    Deny {},
    /// Wire variant `open`.
    #[serde(rename = "open")]
    Open {},
    /// Wire variant `allowlist`.
    #[serde(rename = "allowlist")]
    Allowlist {
        /// Wire field `ids`.
        ids: Vec<String>,
    },
}

/// CMAEG `PartitionRule` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PartitionRule {
    /// Wire field `key`.
    pub key: PartitionKey,
    /// Wire field `version`.
    pub version: u64,
}

/// Provider-owned selection of a provisioned profile or a borrowed existing Agent.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderTarget {
    /// The provider provisions resources from the EventWorkerProfile.
    Profile,
    /// Every sender is routed to this existing Agent without modifying its resources.
    ExistingAgent {
        /// Existing CMA Agent identifier, owned by the channel owner.
        agent_id: String,
    },
}

/// CMAEG `ProviderBinding` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderBinding {
    /// Omitted selects the legacy profile provisioning behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProviderTarget>,
    /// Wire field `binding_id`.
    pub binding_id: BindingId,
    /// Wire field `level`.
    pub level: ConformanceLevel,
    /// Wire field `slug`.
    pub slug: ProviderSlug,
}

/// CMAEG `ProviderRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// Omitted selects the legacy profile provisioning behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProviderTarget>,
    /// Wire field `slug`.
    pub slug: ProviderSlug,
}

/// CMAEG `ProviderResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// Wire field `binding_id`.
    pub binding_id: String,
    /// Wire field `honoured`.
    pub honoured: Vec<String>,
    /// Wire field `level`.
    pub level: ConformanceLevel,
    /// Wire field `refused`.
    pub refused: Vec<FieldRefusal>,
    /// Wire field `slug`.
    pub slug: String,
}

/// CMAEG `PublicChannelDefinition` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicChannelDefinition {
    /// Opaque connection retained by the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    /// Wire field `created_at`.
    pub created_at: String,
    /// Wire field `creation`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation: Option<ChannelCreation>,
    /// Wire field `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Wire field `owner`.
    pub owner: NyxIdIdentity,
    /// Wire field `partition`.
    pub partition: PartitionRule,
    /// Wire field `profile`.
    pub profile: EventWorkerProfile,
    /// Wire field `profile_digest`.
    pub profile_digest: String,
    /// Wire field `provider`.
    pub provider: ProviderBinding,
    /// Wire field `record_id`.
    pub record_id: RecordId,
    /// Wire field `reply`.
    pub reply: ReplyPolicy,
    /// Wire field `schema_version`.
    pub schema_version: u64,
    /// Wire field `secrets`.
    pub secrets: PublicSecrets,
    /// Wire field `sources`.
    pub sources: Vec<Source>,
    /// Wire field `version`.
    pub version: u64,
}

/// CMAEG `PublicSecrets` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicSecrets {
    /// Wire field `agent_key_hash`.
    pub agent_key_hash: String,
}

/// CMAEG `RekeyResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RekeyResponse {
    /// Wire field `kid`.
    pub kid: u64,
}

/// CMAEG `ReplyPolicy` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyPolicy {
    /// Wire field `ack_after_ms`.
    pub ack_after_ms: u64,
    /// Wire field `auto_final`.
    pub auto_final: bool,
    /// Wire field `sinks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sinks: Option<Vec<Sink>>,
}

/// CMAEG `Sink` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Sink {
    /// Wire variant `source`.
    #[serde(rename = "source")]
    Source {},
    /// Wire variant `result_webhook`.
    #[serde(rename = "result_webhook")]
    ResultWebhook {
        /// Wire field `hmac_key_id`.
        hmac_key_id: String,
        /// Wire field `url`.
        url: String,
    },
    /// Wire variant `sync`.
    #[serde(rename = "sync")]
    Sync {},
}

/// CMAEG `Source` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Source {
    /// Native Telegram ingress owned by the gateway.
    #[serde(rename = "telegram")]
    Telegram {
        /// Telegram bot identifier.
        bot_id: String,
        /// Telegram bot username when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `admission`.
        admission: AdmissionPolicy,
        /// Wire field `bot_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
        /// Wire field `bot_username`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        /// Wire field `issuer`.
        issuer: String,
        /// Wire field `jwks_url`.
        jwks_url: String,
        /// Wire field `key_id`.
        key_id: String,
        /// Wire field `platform`.
        platform: Platform,
        /// Wire field `route_ids`.
        route_ids: Vec<String>,
        /// Wire field `trust_normalized`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trust_normalized: Option<bool>,
    },
    /// Wire variant `custom`.
    #[serde(rename = "custom")]
    Custom {
        /// Wire field `auth`.
        auth: CustomAuth,
        /// Wire field `prompt_template`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_template: Option<String>,
        /// Wire field `schema`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<serde_json::Value>,
    },
}

/// CMAEG `SourceRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SourceRequest {
    /// Native Telegram ingress owned by the gateway.
    #[serde(rename = "telegram")]
    Telegram {
        /// Telegram bot identifier.
        bot_id: String,
        /// Telegram bot username when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `admission`.
        admission: AdmissionPolicy,
        /// Wire field `bot_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
        /// Wire field `bot_username`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        /// Wire field `issuer`.
        issuer: String,
        /// Wire field `key_id`.
        key_id: String,
        /// Wire field `platform`.
        platform: Platform,
        /// Wire field `route_ids`.
        route_ids: Vec<String>,
        /// Wire field `trust_normalized`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trust_normalized: Option<bool>,
    },
    /// Wire variant `custom`.
    #[serde(rename = "custom")]
    Custom {
        /// Wire field `auth`.
        auth: CustomAuth,
        /// Wire field `prompt_template`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_template: Option<String>,
        /// Wire field `schema`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<serde_json::Value>,
    },
}

/// CMAEG `TestChannelResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TestChannelResponse {
    /// Wire field `accepted_after_ms`.
    pub accepted_after_ms: u64,
    /// Wire field `first_text`.
    pub first_text: String,
    /// Wire field `level`.
    pub level: ConformanceLevel,
}
