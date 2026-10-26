//! Lark/Feishu media facts from the authenticated raw event, correlated to
//! NyxID's envelope; no network I/O.
//!
//! A relay activity that fails validation is answered 400, which NyxID never
//! retries, so extraction never refuses: a key that cannot be a Lark resource
//! key leaves the attachment not retrievable, and every sender-declared fact
//! passes through [`Attachment::normalize_facts`]. Keys, names and durations
//! never reach logs or metric labels.
use super::attachments::{IndexedAttachment, RelayAttachments};
use crate::telemetry::names;
use cmaeg_core::activity::{
    Attachment, AttachmentKind, AttachmentSource, FactRepairs, MediaVariant,
};
use serde_json::Value;

/// The metric `source` label of every relayed attachment.
const SOURCE: &str = "nyxid_relay";
/// The longest `file_key` or `image_key` NyxID's envelope keeps, so the longest
/// key that can ever correlate to a download index.
const MAX_KEY_BYTES: usize = 1_024;

/// The one attachment of a Lark media message.
pub(super) struct LarkMedia {
    pub attachment: Attachment,
    /// The `payload.content.type` NyxID sets for this message type.
    pub nyxid_type: &'static str,
    pub repairs: FactRepairs,
}

/// The raw content key naming the resource, which NyxID copies into the
/// envelope entry's field of the same name.
#[derive(Clone, Copy)]
enum Key {
    Image,
    File,
}

impl Key {
    fn field(self) -> &'static str {
        match self {
            Self::Image => "image_key",
            Self::File => "file_key",
        }
    }

    fn of(self, item: &IndexedAttachment) -> Option<&str> {
        match self {
            Self::Image => item.attachment.image_key.as_deref(),
            Self::File => item.attachment.file_key.as_deref(),
        }
    }
}

/// The attachment of a Lark `image`, `file`, `audio`, `media` or `video` message, or
/// `None` for any other `message_type`. `content` is `event.message.content`
/// parsed from its JSON string; `envelope` is `payload.content.attachments`.
pub(super) fn extract(
    message_type: &str,
    content: &Value,
    envelope: &RelayAttachments,
) -> Option<LarkMedia> {
    // Each type's attachment kind, its bounded metric label, its variant and
    // NyxID's content type.
    let (key_field, kind, label, variant, nyxid_type) = match message_type {
        "image" => (
            Key::Image,
            AttachmentKind::Image,
            "image",
            MediaVariant::Image,
            "image",
        ),
        "file" => (
            Key::File,
            AttachmentKind::File,
            "file",
            MediaVariant::File,
            "file",
        ),
        "audio" => (
            Key::File,
            AttachmentKind::Audio,
            "audio",
            MediaVariant::Audio,
            "audio",
        ),
        // The cover `image_key` is ignored: NyxID cannot serve it.
        "media" | "video" => (
            Key::File,
            AttachmentKind::Video,
            "video",
            MediaVariant::Media,
            "video",
        ),
        _ => return None,
    };
    let key = lark_key(content.get(key_field.field()));
    let item = key.and_then(|key| {
        envelope
            .kept
            .iter()
            .find(|item| key_field.of(item) == Some(key))
    });
    let mut attachment = Attachment {
        reference: key.map_or_else(|| "lark:item-0".to_owned(), |key| format!("lark:{key}")),
        kind,
        filename: None,
        mime_type: item.and_then(|item| item.attachment.mime_type.clone()),
        size_bytes: item.and_then(|item| item.attachment.size_bytes),
        source: item.map(|item| AttachmentSource::NyxidRelay { index: item.index }),
        variant: Some(variant),
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    };
    if matches!(message_type, "file" | "media" | "video") {
        attachment.filename = match content.get("file_name") {
            Some(Value::String(name)) => Some(name.clone()),
            _ => item.and_then(|item| item.attachment.filename.clone()),
        };
    }
    if matches!(message_type, "audio" | "media" | "video") {
        attachment.duration_seconds = duration_seconds(content.get("duration"));
    }
    count_locator(key.is_some(), item.is_some(), label);
    let repairs = attachment.normalize_facts();
    for field in repairs.fields() {
        metrics::counter!(names::MEDIA_FACTS_REPAIRED, names::SOURCE => SOURCE, names::FIELD => field)
            .increment(1);
    }
    metrics::counter!(
        names::MEDIA_ITEMS,
        names::SOURCE => SOURCE,
        names::KIND => label,
        names::OUTCOME => "extracted"
    )
    .increment(1);
    Some(LarkMedia {
        attachment,
        nyxid_type,
        repairs,
    })
}

/// A key that cannot be a Lark resource key was never correlated, so it counts
/// as an invalid locator rather than as an unmatched correlation.
fn count_locator(keyed: bool, matched: bool, label: &'static str) {
    if !keyed {
        metrics::counter!(
            names::MEDIA_ITEMS,
            names::SOURCE => SOURCE,
            names::KIND => label,
            names::OUTCOME => "invalid_locator"
        )
        .increment(1);
        return;
    }
    let outcome = if matched { "matched" } else { "unmatched" };
    metrics::counter!(names::MEDIA_RELAY_CORRELATION, names::OUTCOME => outcome).increment(1);
}

/// A Lark resource key: a JSON string of 1..=1024 bytes, every byte ASCII
/// graphic. Whitespace, controls and non-ASCII never occur in a real key, and
/// the key is rendered into the stable `lark:<key>` reference.
pub(super) fn lark_key(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|key| {
        (1..=MAX_KEY_BYTES).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

/// Lark declares durations in milliseconds. A partial second rounds up, so a
/// short clip never reads as 0:00; a value too large for the fact saturates and
/// is then dropped by normalization rather than clamped into an invented one.
fn duration_seconds(value: Option<&Value>) -> Option<u32> {
    let milliseconds = value?.as_u64()?;
    Some(u32::try_from(milliseconds.div_ceil(1_000)).unwrap_or(u32::MAX))
}

#[cfg(test)]
#[path = "lark_media_tests.rs"]
mod tests;
