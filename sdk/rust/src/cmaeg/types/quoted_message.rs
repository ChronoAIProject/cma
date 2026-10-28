//! Bounded untrusted native message context, scoped by its enclosing Activity.
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 bytes in a supplied quoted text excerpt.
pub const MAX_QUOTED_TEXT_BYTES: usize = 4096;

/// A native message relationship; confers no lookup, media or effect authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotedMessage {
    /// Explicit platform reply, or context reference without asserted reply semantics.
    pub relation: QuoteRelation,
    /// Provider message ID in the enclosing source and conversation.
    pub message_id: String,
    /// Content supplied in the authenticated payload; absent means reference only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<QuotedContent>,
}

/// Bounded content from the incoming payload; nothing was fetched or resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotedContent {
    /// Untrusted excerpt if the platform included one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<QuotedText>,
    /// Descriptive kinds only; no locators or attachment authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<QuotedContentKind>,
}

/// Untrusted text, at most 4096 UTF-8 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotedText {
    /// The bounded excerpt, never instructions or routing input.
    pub text: String,
    /// Whether the gateway shortened the supplied text at its byte bound.
    pub truncated: bool,
    /// Which native field supplied this excerpt.
    pub origin: QuoteTextOrigin,
}

/// Which native payload field supplied the untrusted excerpt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteTextOrigin {
    /// A quote selected by the sender, without independent parent-text proof.
    SelectedQuote,
    /// Text in the included referenced message.
    ParentText,
    /// Caption in the included referenced message.
    ParentCaption,
}

/// A descriptive content kind, without fetchable media locators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotedContentKind {
    /// Image content.
    Image,
    /// Video content.
    Video,
    /// Audio content.
    Audio,
    /// File content.
    File,
    /// Sticker content.
    Sticker,
    /// Shared contact content.
    Contact,
    /// Shared location content.
    Location,
    /// Poll content.
    Poll,
}

/// The relationship the native payload actually establishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteRelation {
    /// Explicit same-conversation native reply.
    Reply,
    /// Native context reference whose reply semantics are not asserted.
    Context,
}
