use serde::Deserialize;

use super::{CallbackError, InboxAttachment, InboxEnvelope};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    pub message_id: String,
    pub correlation_id: String,
    pub platform: String,
    pub reply_token: Option<String>,
    pub agent: Agent,
    pub conversation: Conversation,
    pub sender: Sender,
    pub content: Content,
    pub reply_to_message_id: Option<String>,
    pub reply_to_platform_message_id: Option<String>,
    pub thread_id: Option<String>,
    pub timestamp: String,
    #[serde(rename = "raw_platform_data")]
    _raw_platform_data: Option<serde::de::IgnoredAny>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Agent {
    pub api_key_id: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Conversation {
    pub id: String,
    pub platform_id: String,
    #[serde(rename = "type")]
    conversation_type: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sender {
    pub platform_id: String,
    display_name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Content {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
    #[serde(default)]
    attachments: Vec<InboxAttachment>,
}

pub(super) fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn optional(value: &Option<String>, limit: usize) -> bool {
    value
        .as_ref()
        .is_none_or(|s| !s.is_empty() && s.len() <= limit && !s.chars().any(char::is_control))
}

impl Payload {
    pub fn validate(&self) -> Result<(), CallbackError> {
        if [
            &self.message_id,
            &self.correlation_id,
            &self.platform,
            &self.agent.api_key_id,
            &self.conversation.id,
            &self.conversation.platform_id,
            &self.conversation.conversation_type,
            &self.sender.platform_id,
            &self.content.content_type,
        ]
        .iter()
        .any(|s| !identifier(s))
            || self.agent.name.len() > 256
            || !optional(&self.sender.display_name, 256)
            || !optional(&self.reply_to_message_id, 256)
            || !optional(&self.reply_to_platform_message_id, 256)
            || !optional(&self.thread_id, 4096)
            || self.timestamp.len() > 64
            || crate::bridge::provider_timestamp(&self.timestamp).is_none()
            || self.reply_token.as_ref().is_some_and(|s| s.len() > 16384)
            || self.content.text.as_ref().is_some_and(|s| s.len() > 524288)
            || self.content.attachments.len() > 32
        {
            return Err(CallbackError::Bounds);
        }
        for attachment in &self.content.attachments {
            if !identifier(&attachment.content_type)
                || attachment.url.len() > 4096
                || !optional(&attachment.platform_message_id, 256)
                || !optional(&attachment.file_key, 1024)
                || !optional(&attachment.image_key, 1024)
                || !optional(&attachment.filename, 512)
                || !optional(&attachment.mime_type, 256)
                || attachment.size_bytes.is_some_and(|s| s > i64::MAX as u64)
            {
                return Err(CallbackError::Bounds);
            }
            // Stored references are inert. Download adapters need an independent
            // policy and must never interpret these URLs as fetch authority.
            if !attachment.url.is_empty() {
                let url = url::Url::parse(&attachment.url).map_err(|_| CallbackError::Payload)?;
                if !matches!(url.scheme(), "https" | "http")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(CallbackError::Payload);
                }
            }
        }
        Ok(())
    }

    pub fn normalize(self) -> InboxEnvelope {
        InboxEnvelope {
            platform: self.platform,
            route_id: self.conversation.id,
            chat_id: self.conversation.platform_id,
            conversation_type: self.conversation.conversation_type,
            sender_id: self.sender.platform_id,
            sender_display_name: self.sender.display_name,
            content_type: self.content.content_type,
            text: self.content.text,
            attachments: self.content.attachments,
            reply_to_message_id: self.reply_to_message_id,
            reply_to_platform_message_id: self.reply_to_platform_message_id,
            protected_thread_context_present: false,
            timestamp: self.timestamp,
        }
    }
}
