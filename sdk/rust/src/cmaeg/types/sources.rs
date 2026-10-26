//! Source wire types for the independent CMAEG v1 protocol.
use super::{AdmissionPolicy, CustomAuth, Platform};
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
    /// Native Discord Gateway source; credentials remain in the encrypted channel.
    #[serde(rename = "discord")]
    Discord {
        /// Exact bot identity.
        bot_id: String,
        /// Requested supported Gateway intents, including MESSAGE_CONTENT.
        intents: u64,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
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
        /// Opt into raw Discord/WhatsApp event verification; never combine with trust_normalized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify_raw_event: Option<bool>,
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
    /// Native Discord Gateway source; credentials remain in the encrypted channel.
    #[serde(rename = "discord")]
    Discord {
        /// Exact bot identity.
        bot_id: String,
        /// Requested supported Gateway intents, including MESSAGE_CONTENT.
        intents: u64,
        /// Event admission policy.
        admission: AdmissionPolicy,
    },
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
        /// Opt into raw Discord/WhatsApp event verification; never combine with trust_normalized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verify_raw_event: Option<bool>,
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
