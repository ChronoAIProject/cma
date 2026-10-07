//! Strict, bounded NyxID callback envelope; raw facts remain in request memory only.
use super::VerificationError;
use cmaeg_core::channel::Secret;
use serde::Deserialize;
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = CallbackPayload)]
pub struct CallbackPayload {
    pub message_id: String,
    pub correlation_id: String,
    pub platform: String,
    #[schema(required = true, nullable = false)]
    pub reply_token: Option<Secret>,
    pub agent: Agent,
    pub conversation: Conversation,
    pub sender: Sender,
    pub content: Content,
    pub reply_to_message_id: Option<String>,
    pub reply_to_platform_message_id: Option<String>,
    pub thread_id: Option<Secret>,
    pub timestamp: String,
    pub raw_platform_data: Option<Value>,
    /// Additive NyxID hint; raw platform verification remains authoritative.
    #[serde(default)]
    pub thread_context: Option<Value>,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelayAgent)]
pub struct Agent {
    pub api_key_id: String,
    pub name: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelayConversation)]
pub struct Conversation {
    pub id: String,
    pub platform_id: String,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelaySender)]
pub struct Sender {
    pub platform_id: String,
    pub display_name: Option<String>,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelayContent)]
pub struct Content {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelayAttachment)]
pub struct Attachment {
    pub content_type: String,
    pub url: String,
    pub platform_message_id: Option<String>,
    pub file_key: Option<String>,
    pub image_key: Option<String>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
}
pub(super) fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn optional(value: Option<&str>, max: usize) -> bool {
    value.is_none_or(|value| {
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
    })
}
pub fn timestamp(value: &str) -> Option<i64> {
    if value.len() > 64 {
        return None;
    }
    let seconds = OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .unix_timestamp();
    (seconds >= 0).then_some(seconds)
}
impl CallbackPayload {
    pub fn validate(&self) -> Result<(), VerificationError> {
        if [
            &self.message_id,
            &self.correlation_id,
            &self.platform,
            &self.agent.api_key_id,
            &self.conversation.id,
            &self.conversation.platform_id,
            &self.conversation.kind,
            &self.sender.platform_id,
            &self.content.kind,
        ]
        .iter()
        .any(|value| !identifier(value))
            || self.agent.name.len() > 256
            || !optional(self.sender.display_name.as_deref(), 256)
            || !optional(self.reply_to_message_id.as_deref(), 256)
            || !optional(self.reply_to_platform_message_id.as_deref(), 256)
            || !optional(self.thread_id.as_ref().map(Secret::as_str), 4096)
            || timestamp(&self.timestamp).is_none()
            || self
                .reply_token
                .as_ref()
                .is_some_and(|token| token.as_str().len() > 16_384)
            || self
                .content
                .text
                .as_ref()
                .is_some_and(|text| text.len() > 524_288)
            || self.content.attachments.len() > 32
        {
            return Err(VerificationError("payload_bounds"));
        }
        for attachment in &self.content.attachments {
            if !identifier(&attachment.content_type)
                || attachment.url.len() > 4096
                || !optional(attachment.platform_message_id.as_deref(), 256)
                || !optional(attachment.file_key.as_deref(), 1024)
                || !optional(attachment.image_key.as_deref(), 1024)
                || !optional(attachment.filename.as_deref(), 512)
                || !optional(attachment.mime_type.as_deref(), 256)
                || attachment
                    .size_bytes
                    .is_some_and(|size| size > i64::MAX as u64)
            {
                return Err(VerificationError("payload_bounds"));
            }
            if !attachment.url.is_empty() {
                let url = reqwest::Url::parse(&attachment.url)
                    .map_err(|_| VerificationError("attachment_url"))?;
                if !matches!(url.scheme(), "https" | "http")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(VerificationError("attachment_url"));
                }
            }
        }
        Ok(())
    }
}
