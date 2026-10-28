//! A native reply relationship is context data, never an address or capability.
use super::SourceKind;
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_QUOTED_TEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotedMessage {
    /// Explicit platform reply, or a context reference whose reply semantics are not asserted.
    pub relation: QuoteRelation,
    /// Provider message ID within the enclosing native source and conversation.
    /// Does not authorize lookup, media fetch, reply, edit, or other effects.
    pub message_id: String,
    /// Only content supplied with this authenticated incoming message. Absent
    /// means the payload supplies the relationship only; nothing was fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<QuotedContent>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotedContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<QuotedText>,
    /// Descriptive kinds only; never attachment locators or fetch authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<QuotedContentKind>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuotedText {
    /// Untrusted user content, at most 4096 UTF-8 bytes.
    pub text: String,
    pub truncated: bool,
    pub origin: QuoteTextOrigin,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuoteTextOrigin {
    SelectedQuote,
    ParentText,
    ParentCaption,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuotedContentKind {
    Image,
    Video,
    Audio,
    File,
    Sticker,
    Contact,
    Location,
    Poll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuoteRelation {
    Reply,
    Context,
}

impl QuotedMessage {
    pub fn validate(&self, source: &SourceKind) -> Result<(), ValidationError> {
        let id = &self.message_id;
        let valid = match source {
            SourceKind::Telegram { .. } => id
                .parse::<i64>()
                .is_ok_and(|n| n > 0 && n.to_string() == *id),
            SourceKind::Discord { .. } => crate::channel::valid_discord_id(id),
            SourceKind::NativeLark(_) => {
                crate::channel::native_lark::safe_id(id) && id.starts_with("om_")
            }
            SourceKind::Whatsapp { .. } => {
                id.starts_with("wamid.")
                    && (7..=512).contains(&id.len())
                    && id.bytes().all(|b| b.is_ascii_graphic())
            }
            _ => false,
        };
        let supported = match source {
            SourceKind::Whatsapp { .. } => {
                self.relation == QuoteRelation::Context && self.content.is_none()
            }
            SourceKind::NativeLark(_) => {
                self.relation == QuoteRelation::Reply && self.content.is_none()
            }
            SourceKind::Telegram { .. } => self.relation == QuoteRelation::Reply,
            SourceKind::Discord { .. } => {
                self.relation == QuoteRelation::Reply
                    && self
                        .content
                        .as_ref()
                        .and_then(|c| c.text.as_ref())
                        .is_none_or(|t| t.origin == QuoteTextOrigin::ParentText)
            }
            _ => false,
        };
        if !valid || !supported {
            return Err(ValidationError::new(
                "/kind/quoted_message/message_id",
                "native scoped message ID required",
            ));
        }
        if let Some(content) = &self.content {
            if content.kinds.len() > 8
                || content
                    .kinds
                    .iter()
                    .enumerate()
                    .any(|(i, k)| content.kinds[..i].contains(k))
            {
                return Err(ValidationError::new(
                    "/kind/quoted_message/content/kinds",
                    "at most eight distinct descriptive content kinds",
                ));
            }
            if let Some(text) = &content.text {
                crate::require_string(
                    &text.text,
                    "/kind/quoted_message/content/text/text",
                    0,
                    MAX_QUOTED_TEXT_BYTES,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn quoted_context_bounds_and_source_semantics_are_strict() {
        let telegram = SourceKind::Telegram {
            platform: crate::channel::Platform::Telegram,
            bot_id: "77".into(),
        };
        let valid = json!({"relation":"reply","message_id":"8","content":{"text":{"text":"passage","truncated":false,"origin":"selected_quote"},"kinds":["image"]}});
        let mut quote: QuotedMessage = serde_json::from_value(valid.clone()).unwrap();
        quote.validate(&telegram).unwrap();
        quote.content.as_mut().unwrap().text.as_mut().unwrap().text =
            "x".repeat(MAX_QUOTED_TEXT_BYTES + 1);
        assert!(quote.validate(&telegram).is_err());
        let mut invalid = valid.clone();
        invalid["message_ref"] = "not-authority".into();
        assert!(serde_json::from_value::<QuotedMessage>(invalid).is_err());
        let quote: QuotedMessage = serde_json::from_value(valid).unwrap();
        assert!(quote.validate(&SourceKind::Custom).is_err());
        assert!(quote
            .validate(&SourceKind::Discord {
                bot_id: "77".into()
            })
            .is_err());
        let whatsapp = SourceKind::Whatsapp {
            app_id: "1".into(),
            waba_id: "2".into(),
            phone_number_id: "3".into(),
        };
        let mut context = QuotedMessage {
            relation: QuoteRelation::Context,
            message_id: "wamid.parent".into(),
            content: None,
        };
        context.validate(&whatsapp).unwrap();
        context.content = quote.content;
        assert!(context.validate(&whatsapp).is_err());
    }
}
