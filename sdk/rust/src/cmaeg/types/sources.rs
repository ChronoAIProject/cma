//! Source wire types for the independent CMAEG v1 protocol.
use super::{AdmissionPolicy, CustomAuth, Platform};
use serde::{Deserialize, Serialize};

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
