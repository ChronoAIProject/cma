//! Management request shapes, separate from construction and validation.
use cmaeg_core::{
    channel::{AdmissionPolicy, CustomAttachments, CustomAuth, ReplyPolicy, Secret},
    ids::{ProviderSlug, RecordId},
    profile::EventWorkerProfile,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// Request wrapper used by channel creation.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreateRequest {
    /// Stable UUID reserved by a durable creator before issuing the request.
    /// Required as a canonical UUID v4 when authoring native Discord or WhatsApp;
    /// optional for existing non-native creation contracts.
    #[serde(default)]
    pub record_id: Option<RecordId>,
    pub schema_version: u16,
    pub name: String,
    pub profile: EventWorkerProfile,
    pub provider: ProviderRequest,
    pub sources: Vec<SourceRequest>,
    #[serde(default)]
    pub connection_ref: Option<String>,
    #[serde(default)]
    pub connection_admission: Option<AdmissionPolicy>,
    pub partition: cmaeg_core::channel::PartitionRule,
    pub reply: ReplyPolicy,
    pub credentials: CredentialsRequest,
}

/// Update body; credentials are retained when this field is absent.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelUpdateRequest {
    pub schema_version: u16,
    pub name: String,
    pub profile: EventWorkerProfile,
    pub provider: ProviderRequest,
    pub sources: Vec<SourceRequest>,
    #[serde(default)]
    pub connection_ref: Option<String>,
    #[serde(default)]
    pub connection_admission: Option<AdmissionPolicy>,
    pub partition: cmaeg_core::channel::PartitionRule,
    pub reply: ReplyPolicy,
    #[serde(default)]
    pub credentials: Option<CredentialPatch>,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequest {
    pub slug: ProviderSlug,
    #[serde(
        default,
        skip_serializing_if = "cmaeg_core::channel::ProviderTarget::is_profile"
    )]
    pub target: cmaeg_core::channel::ProviderTarget,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum SourceRequest {
    /// Requires a separate single-tenant native app; never replace a NyxID webhook.
    NativeLark(cmaeg_core::channel::NativeLarkSource),
    /// Native Meta WhatsApp Cloud API; independent of NyxID relay sources.
    Whatsapp {
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
        /// Retain original message routing for edits and attributable reactions.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        message_events: bool,
        /// Admit native channel posts as non-human channel identities; requires message_events.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        channel_posts: bool,
        bot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_username: Option<String>,
        admission: AdmissionPolicy,
    },
    /// Native Discord Gateway. New writes require the native Discord write gate.
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
    NyxidRelay {
        issuer: String,
        key_id: String,
        route_ids: Vec<String>,
        platform: cmaeg_core::channel::Platform,
        #[serde(default)]
        bot_username: Option<String>,
        // Omitted when absent so retained creation fingerprints stay stable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bot_id: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        trust_normalized: bool,
        /// Opt into verified Discord/WhatsApp facts without changing retained sources.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        verify_raw_event: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_resources: Option<cmaeg_core::channel::LarkResourceBinding>,
        /// Explicit write authority; independent of resource reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_message_effects: Option<Box<cmaeg_core::channel::LarkMessageEffectsBinding>>,
        admission: AdmissionPolicy,
    },
    Custom {
        auth: CustomAuth,
        #[serde(default)]
        prompt_template: Option<String>,
        #[serde(default)]
        schema: Option<Value>,
        // Omitted when absent so retained creation fingerprints stay stable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<CustomAttachments>,
    },
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialsRequest {
    #[serde(default)]
    pub native_lark: Option<cmaeg_core::channel::NativeLarkCredentials>,
    #[serde(default)]
    pub whatsapp: Option<cmaeg_core::channel::WhatsappCredentials>,
    pub agent_key: Secret,
    #[serde(default)]
    pub channel_key: Option<Secret>,
    #[serde(default)]
    pub telegram_bot_token: Option<Secret>,
    #[serde(default)]
    pub discord_bot_token: Option<Secret>,
    #[serde(default)]
    pub telegram_webhook_secret: Option<Secret>,
    #[serde(default)]
    pub hmac_secrets: Vec<HmacSecretRequest>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialPatch {
    #[serde(default)]
    pub native_lark: Option<cmaeg_core::channel::NativeLarkCredentials>,
    #[serde(default)]
    pub whatsapp: Option<cmaeg_core::channel::WhatsappCredentials>,
    #[serde(default)]
    pub agent_key: Option<Secret>,
    #[serde(default)]
    pub channel_key: Option<Secret>,
    #[serde(default)]
    pub telegram_bot_token: Option<Secret>,
    #[serde(default)]
    pub discord_bot_token: Option<Secret>,
    #[serde(default)]
    pub telegram_webhook_secret: Option<Secret>,
    #[serde(default)]
    pub hmac_secrets: Option<Vec<HmacSecretRequest>>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HmacSecretRequest {
    pub key_id: String,
    pub secret: Secret,
}
