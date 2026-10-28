//! Source wire types for the independent CMAEG v1 protocol.
use super::{AdmissionPolicy, CustomAuth, Platform, Secret};
use serde::{Deserialize, Serialize};

/// CMAEG `CustomAttachments` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomAttachments {
    /// Wire field `origins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origins: Option<Vec<String>>,
    /// Wire field `pointer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
}

/// CMAEG `Source` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Source {
    /// Separate native app, independent of NyxID relay sources.
    #[serde(rename = "native_lark")]
    NativeLark(NativeLarkSource),
    /// Native Meta WhatsApp Cloud API, independent of relay sources.
    #[serde(rename = "whatsapp")]
    Whatsapp {
        /// Meta app identity proved during source authoring.
        app_id: String,
        /// WhatsApp Business Account owning the configured phone.
        waba_id: String,
        /// Native receiving and sending phone number identity.
        phone_number_id: String,
        /// Explicit owner consent to create and publish managed static WhatsApp Flows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forms_enabled: Option<bool>,
        /// Explicit existing-group opt-in for an Official Business Account phone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_ids: Option<Vec<String>>,
        /// Admission policy for original participants and private/group conversations.
        admission: AdmissionPolicy,
    },
    /// Native Discord Gateway source; credentials remain in the encrypted channel.
    #[serde(rename = "discord")]
    Discord {
        /// Exact bot identity.
        bot_id: String,
        /// Requested supported Gateway intents, including MESSAGE_CONTENT.
        intents: u64,
        /// Explicitly allow a bounded read of the event’s earlier thread messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_history: Option<bool>,
        /// Opt in to bounded original-event thread lifecycle operations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_lifecycle: Option<bool>,
        /// Read earlier messages and attachment facts in the original conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_history: Option<bool>,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
    /// Native Telegram ingress owned by the gateway.
    #[serde(rename = "telegram")]
    Telegram {
        /// Opt in to close/reopen of the exact original forum topic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic_lifecycle: Option<bool>,
        /// Retain original message routing for edits and attributable reactions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_events: Option<bool>,
        /// Admit native channel posts with unknown channel actors; requires message_events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel_posts: Option<bool>,
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
        /// Opt into raw Discord/WhatsApp event verification; never combine with trust_normalized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify_raw_event: Option<bool>,
        /// Opt-in exact Lark/Feishu resource proxy binding.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_resources: Option<LarkResourceBinding>,
        /// Explicit write authority; independent of resource reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_message_effects: Option<Box<LarkMessageEffectsBinding>>,
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
        /// Wire field `attachments`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<CustomAttachments>,
    },
}

/// CMAEG `SourceRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SourceRequest {
    /// Separate native app, independent of NyxID relay sources.
    #[serde(rename = "native_lark")]
    NativeLark(NativeLarkSource),
    /// Native Meta WhatsApp Cloud API, independent of relay sources.
    #[serde(rename = "whatsapp")]
    Whatsapp {
        /// Meta app identity proved during source authoring.
        app_id: String,
        /// WhatsApp Business Account owning the configured phone.
        waba_id: String,
        /// Native receiving and sending phone number identity.
        phone_number_id: String,
        /// Explicit owner consent to create and publish managed static WhatsApp Flows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forms_enabled: Option<bool>,
        /// Explicit existing-group opt-in for an Official Business Account phone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_ids: Option<Vec<String>>,
        /// Admission policy for original participants and private/group conversations.
        admission: AdmissionPolicy,
    },
    /// Native Discord Gateway source; credentials remain in the encrypted channel.
    #[serde(rename = "discord")]
    Discord {
        /// Exact bot identity.
        bot_id: String,
        /// Requested supported Gateway intents, including MESSAGE_CONTENT.
        intents: u64,
        /// Explicitly allow a bounded read of the event’s earlier thread messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_history: Option<bool>,
        /// Opt in to bounded original-event thread lifecycle operations.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_lifecycle: Option<bool>,
        /// Read earlier messages and attachment facts in the original conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_history: Option<bool>,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
    /// Native Telegram ingress owned by the gateway.
    #[serde(rename = "telegram")]
    Telegram {
        /// Opt in to close/reopen of the exact original forum topic.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic_lifecycle: Option<bool>,
        /// Retain original message routing for edits and attributable reactions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_events: Option<bool>,
        /// Admit native channel posts with unknown channel actors; requires message_events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel_posts: Option<bool>,
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
        /// Opt into raw Discord/WhatsApp event verification; never combine with trust_normalized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify_raw_event: Option<bool>,
        /// Exact authorized connection for opt-in embedded Lark/Feishu media.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_resources: Option<LarkResourceBinding>,
        /// Explicit write authority; independent of resource reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lark_message_effects: Option<Box<LarkMessageEffectsBinding>>,
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
        /// Wire field `attachments`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<CustomAttachments>,
    },
}

/// CMAEG `SourceKind` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SourceKind {
    /// Separate native app, independent of NyxID relay sources.
    #[serde(rename = "native_lark")]
    NativeLark(NativeLarkIdentity),
    /// Native Meta app/account/phone identities.
    #[serde(rename = "whatsapp")]
    Whatsapp {
        /// Meta app identity proved during source authoring.
        app_id: String,
        /// WhatsApp Business Account owning the configured phone.
        waba_id: String,
        /// Native receiving and sending phone number identity.
        phone_number_id: String,
        /// Explicit owner consent to create and publish managed static WhatsApp Flows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forms_enabled: Option<bool>,
    },
    /// Native Discord ingress.
    #[serde(rename = "discord")]
    Discord {
        /// Exact receiving bot identity.
        bot_id: String,
    },
    /// Native Telegram ingress.
    #[serde(rename = "telegram")]
    Telegram {
        /// Source platform.
        platform: Platform,
        /// Telegram bot identifier.
        bot_id: String,
    },
    /// Wire variant `nyxid_relay`.
    #[serde(rename = "nyxid_relay")]
    NyxidRelay {
        /// Wire field `platform`.
        platform: Platform,
        /// Wire field `route_id`.
        route_id: String,
    },
    /// Wire variant `custom`.
    #[serde(rename = "custom")]
    Custom {},
}

/// Explicit scoped connection for embedded Lark/Feishu media; contains no credentials.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LarkResourceBinding {
    /// Exact authorized NyxID UserService identifier.
    pub user_service_id: String,
    /// Exact regional Lark/Feishu catalog service identifier.
    pub catalog_service_id: String,
    /// Independently configured receiving application ID (`cli_…`).
    pub app_id: String,
}

/// Independent opt-in to effects on confirmed CMA Lark/Feishu replies.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LarkMessageEffectsBinding {
    /// Exact authorized regional bot-service connection.
    pub connection: LarkResourceBinding,
    /// Permit deletion of exact confirmed outbound messages.
    #[serde(default)]
    pub delete: bool,
    /// Permit adding, replacing and clearing the reaction retained by CMA.
    #[serde(default)]
    pub reaction: bool,
}

/// Native regional identity, scoped to one separately provisioned custom app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeLarkIdentity {
    /// Regional API and app home.
    pub region: LarkRegion,
    /// Custom app ID.
    pub app_id: String,
    /// Exact receiving tenant key.
    pub tenant_key: String,
    /// App-scoped bot open ID, independently verified.
    pub bot_open_id: String,
}
/// Native API region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LarkRegion {
    /// International Lark.
    Lark,
    /// China Feishu.
    Feishu,
}
/// Separate native source. Configure a new app; never replace a NyxID webhook.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeLarkSource {
    /// Explicit consent to original chat/thread history, including other participants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_history: Option<bool>,
    /// Must be true to acknowledge separate app provisioning.
    pub separate_app: bool,
    /// Exact app, tenant, region and receiving bot.
    pub identity: NativeLarkIdentity,
    /// Original sender/chat and human reaction admission.
    pub admission: AdmissionPolicy,
}

/// Secrets supplied only to native app authoring; never present in public definitions.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeLarkCredentials {
    /// Custom app secret.
    pub app_secret: Secret,
    /// Required encrypted callback key.
    pub encrypt_key: Secret,
    /// Required callback verification token.
    pub verification_token: Secret,
}

/// Owner-visible native Discord activation state. Future state identifiers are retained.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeDiscordActivation {
    /// Selected, pending_discovery, capacity_exceeded, or a configuration refusal.
    pub state: String,
    /// Safe explanation without credentials or other channel identities.
    pub detail: String,
    /// Maximum records selected by this process.
    pub capacity: usize,
    /// Current process selection count, not a socket count.
    pub selected_records: usize,
}
