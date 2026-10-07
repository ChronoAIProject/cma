//! Relay wire types for the independent CMAEG v1 protocol.
use super::{Refusal, Secret};
use serde::{Deserialize, Serialize};

/// CMAEG `CallbackPayload` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CallbackPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional normalized hint; the gateway verifies raw platform data instead.
    pub thread_context: Option<serde_json::Value>,
    /// Wire field `agent`.
    pub agent: RelayAgent,
    /// Wire field `content`.
    pub content: RelayContent,
    /// Wire field `conversation`.
    pub conversation: RelayConversation,
    /// Wire field `correlation_id`.
    pub correlation_id: String,
    /// Wire field `message_id`.
    pub message_id: String,
    /// Wire field `platform`.
    pub platform: String,
    /// Wire field `raw_platform_data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_platform_data: Option<serde_json::Value>,
    /// Wire field `reply_to_message_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    /// Wire field `reply_to_platform_message_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_platform_message_id: Option<String>,
    /// Wire field `reply_token`.
    pub reply_token: Secret,
    /// Wire field `sender`.
    pub sender: RelaySender,
    /// Wire field `thread_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Secret>,
    /// Wire field `timestamp`.
    pub timestamp: String,
}

/// CMAEG `CallbackResponse` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum CallbackResponse {
    /// Wire variant `accepted`.
    #[serde(rename = "accepted")]
    Accepted {
        /// Wire field `conversation_id`.
        conversation_id: String,
        /// Wire field `event_id`.
        event_id: String,
    },
    /// Wire variant `refused`.
    #[serde(rename = "refused")]
    Refused {
        /// Wire field `reason`.
        reason: Refusal,
    },
}

/// CMAEG `RelayAgent` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAgent {
    /// Wire field `api_key_id`.
    pub api_key_id: String,
    /// Wire field `name`.
    pub name: String,
}

/// CMAEG `RelayAttachment` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAttachment {
    /// Wire field `content_type`.
    pub content_type: String,
    /// Wire field `file_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_key: Option<String>,
    /// Wire field `filename`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Wire field `image_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_key: Option<String>,
    /// Wire field `mime_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Wire field `platform_message_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_message_id: Option<String>,
    /// Wire field `size_bytes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Wire field `url`.
    pub url: String,
}

/// CMAEG `RelayContent` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayContent {
    /// Wire field `attachments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<RelayAttachment>>,
    /// Wire field `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Wire field `type`.
    pub r#type: String,
}

/// CMAEG `RelayConversation` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayConversation {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `platform_id`.
    pub platform_id: String,
    /// Wire field `type`.
    pub r#type: String,
}

/// CMAEG `RelaySender` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RelaySender {
    /// Wire field `display_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Wire field `platform_id`.
    pub platform_id: String,
}
