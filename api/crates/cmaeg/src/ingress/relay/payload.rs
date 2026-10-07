//! Strict, bounded NyxID callback envelope; raw facts remain in request memory only.
//! Attachment items are the one lenient part: see [`super::attachments`].
use super::{attachments::RelayAttachments, VerificationError};
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
    // NyxID's thread facts, dormant for CMAEG routes today. Accepted and ignored
    // so a NyxID-side enablement cannot turn every callback into a 401 that NyxID
    // never retries; the raw-event observers stay authoritative for threads, and
    // the request body cap bounds the value.
    #[serde(default)]
    #[schema(value_type = Option<serde_json::Value>)]
    pub thread_context: Option<serde::de::IgnoredAny>,
    // NyxID's activity metadata, dormant for CMAEG routes today and ignored for
    // the same reason; admission relies only on the raw platform event.
    #[serde(default)]
    #[schema(value_type = Option<serde_json::Value>)]
    pub activity: Option<serde::de::IgnoredAny>,
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
    #[schema(value_type = Vec<Attachment>)]
    pub attachments: RelayAttachments,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RelayAttachment)]
pub struct Attachment {
    pub content_type: String,
    /// NyxID's own download endpoint for this attachment. Accepted for envelope
    /// compatibility and never dereferenced: media fetches build their URL from
    /// CMAEG_NYXID_BASE_URL and the sealed message ID.
    #[serde(default)]
    pub download_url: Option<String>,
    // Opaque provider handle (a Telegram file_id or a Lark resource URL). CMAEG
    // never parses or dereferences it; retrieval uses the sealed message ID and
    // the attachment index. A plain comment, so the published schema is unchanged.
    pub url: String,
    pub platform_message_id: Option<String>,
    pub file_key: Option<String>,
    pub image_key: Option<String>,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
}
const MAX_DISPLAY_NAME: usize = 256;
impl Sender {
    /// NyxID concatenates the sender's first and last name (up to 129 characters,
    /// 387 bytes in CJK), and rejecting an oversized name lost the whole message.
    /// Control characters are removed, the name is truncated to 256 bytes on a char
    /// boundary, and an empty result becomes `None`. Returns whether it changed.
    pub fn repair_display_name(&mut self) -> bool {
        let Some(original) = self.display_name.take() else {
            return false;
        };
        let mut name: String = original.chars().filter(|c| !c.is_control()).collect();
        name.truncate(name.floor_char_boundary(MAX_DISPLAY_NAME));
        let changed = name.is_empty() || name.len() != original.len();
        self.display_name = (!name.is_empty()).then_some(name);
        changed
    }
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
    /// Raw attachment positions, including dropped ones; presence checks use it so
    /// a malformed list is refused rather than admitted as a text-only message.
    pub fn received_attachments(&self) -> u32 {
        self.content.attachments.received
    }
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
            || !optional(self.sender.display_name.as_deref(), MAX_DISPLAY_NAME)
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
        {
            return Err(VerificationError("payload_bounds"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Sender;

    fn repaired(name: Option<&str>) -> (bool, Option<String>) {
        let mut sender = Sender {
            platform_id: "7".into(),
            display_name: name.map(str::to_owned),
        };
        (sender.repair_display_name(), sender.display_name)
    }

    #[test]
    fn valid_and_absent_display_names_are_unchanged() {
        assert_eq!(repaired(None), (false, None));
        assert_eq!(repaired(Some("Alice")), (false, Some("Alice".into())));
        let longest = "a".repeat(256);
        assert_eq!(repaired(Some(&longest)), (false, Some(longest.clone())));
    }

    #[test]
    fn oversized_names_are_truncated_on_a_char_boundary() {
        let original = format!("{} {}", "名".repeat(64), "姓".repeat(64));
        assert_eq!(original.len(), 385);
        let (changed, name) = repaired(Some(&original));
        let name = name.unwrap();
        assert!(changed);
        assert_eq!(name.len(), 256);
        assert!(original.starts_with(&name));
        let (_, name) = repaired(Some(&"名".repeat(86)));
        assert_eq!(name.unwrap().len(), 255);
    }

    #[test]
    fn control_characters_are_removed_and_empty_names_become_absent() {
        assert_eq!(repaired(Some("Al\u{7}ice\n")), (true, Some("Alice".into())));
        assert_eq!(repaired(Some("\u{7}\u{0}\t")), (true, None));
        assert_eq!(repaired(Some("")), (true, None));
    }
}
