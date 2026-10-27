use super::{NativeMediaKind, NativeMediaSource};
use serde::{Deserialize, Deserializer, Serialize};

/// A replacement upload of the original media kind; nested captions are unsupported.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaReplacement {
    /// Must match the retained reference's original kind.
    pub kind: NativeMediaKind,
    /// HTTPS URL or bounded inline bytes.
    pub source: NativeMediaSource,
    /// Filename without path components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// MIME type without parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Explicit single-message media patch. At least one member is required by the server.
/// Omission preserves each value, an empty string clears it, and null is invalid.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaUpdate {
    /// Upload new bytes of the retained original media kind.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub replacement: Option<NativeMediaReplacement>,
    /// Telegram visible caption; Discord attachment description (alt text).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub caption: Option<String>,
    /// Discord visible message content; unsupported on Telegram.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub text: Option<String>,
}

/// Deployment eligibility or exact-reference edit authority, separate from legacy edit/effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeMediaEditCapabilities {
    /// Caption/attachment description edits are supported.
    pub caption: bool,
    /// Visible message content edits are supported.
    pub content: bool,
    /// Same-kind replacements supported by this context or exact reference.
    pub replacement_kinds: Vec<NativeMediaKind>,
    /// Maximum attachment count per referenced platform message.
    pub max_attachments: u8,
    /// Maximum aggregate replacement bytes.
    pub max_bytes: u64,
    /// True when every edit needs a durable caller-chosen key.
    pub idempotency_key_required: bool,
    /// Original reference expiry in Unix seconds; updates never extend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

pub(super) fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
