//! Attachments carried by a message or custom event activity: a stable
//! reference, an optional retrieval locator and optional platform facts.
//!
//! The bounds below are the single definition of the attachment domain. The
//! validator, the fact normalizer and every source's extractor share them, so a
//! value one of them produces can never be refused by another.

use std::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::SourceKind;
use crate::{require_string, ValidationError};

pub const MAX_ATTACHMENTS: usize = 16;
pub const MAX_REFERENCE_BYTES: usize = 4_096;
pub const MAX_FILENAME_BYTES: usize = 255;
pub const MAX_MIME_PART_BYTES: usize = 127;
pub const MAX_DURATION_SECONDS: u32 = 86_400;
pub const MAX_DIMENSION: u32 = 100_000;
pub const MAX_EMOJI_BYTES: usize = 32;
/// NyxID relays at most 32 attachments per callback, addressed by position.
pub const MAX_NYXID_INDEX: u8 = 31;
pub const MAX_FILE_ID_BYTES: usize = 256;
pub const MAX_FILE_UNIQUE_ID_BYTES: usize = 128;
pub const MAX_ATTACHMENT_URL_BYTES: usize = 2_048;
pub const MAX_INLINE_POINTER_BYTES: usize = 256;
pub const MAX_ALBUM_ID_BYTES: usize = 128;

/// One attachment of an activity.
///
/// Every fact is what the source declared. Content derived from the bytes
/// (transcripts, pixels, extracted text) never enters the Activity: the sealed
/// activity digest covers only what the source reported, and derived content is
/// produced on demand from the locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Stable, non-secret identifier such as `telegram:<file_unique_id>`,
    /// `lark:<key>`, `<platform>:item-<i>` or an owner-supplied value.
    pub reference: String,
    pub kind: AttachmentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Where the bytes can be fetched; absent means the attachment is not
    /// retrievable and only its facts are known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AttachmentSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<MediaVariant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// A platform preview from the same native Telegram or Lark message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<AttachmentSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
    Audio,
    Video,
    Other,
}

/// Where an attachment's bytes can be fetched.
///
/// Locators are non-secret and bound into the activity digest, but they never
/// carry retrieval authority: the NyxID message ID stays inside the sealed reply
/// address, and every fetch is authorized by the event_ref that sealed the
/// activity. Each type is valid only with its matching activity source.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum AttachmentSource {
    /// Exact resource of an authenticated native Lark/Feishu message.
    NativeLark {
        message_id: String,
        resource_key: String,
        resource_kind: crate::channel::LarkResourceKind,
        source_revision: String,
    },
    /// Signed native WhatsApp media; temporary download URLs are never retained.
    Whatsapp {
        message_id: String,
        media_id: String,
        /// Canonical lowercase hexadecimal digest from the signed webhook.
        sha256: String,
    },
    /// Exact native Discord message attachment; resolved with current bot custody.
    Discord {
        channel_id: String,
        message_id: String,
        attachment_id: String,
    },
    /// Position of the attachment in the NyxID relay callback.
    NyxidRelay { index: u8 },
    /// Authenticated embedded Lark/Feishu resource, bound to its source authority revision.
    LarkResource {
        message_id: String,
        resource_key: String,
        resource_kind: crate::channel::LarkResourceKind,
        binding: crate::channel::LarkResourceBinding,
        bot_id: String,
        source_revision: String,
    },
    /// Telegram Bot API file identifiers of a natively received message.
    Telegram {
        file_id: String,
        file_unique_id: String,
    },
    /// An absolute HTTPS URL supplied by a custom event source.
    Url { url: String },
    /// An RFC 6901 JSON pointer to bytes carried inside the custom event itself.
    Inline { pointer: String },
}

/// File identifiers and URLs must never reach logs, so a locator prints only
/// its type and the size of each string.
impl fmt::Debug for AttachmentSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NativeLark { .. } => formatter.write_str("NativeLark([REDACTED])"),
            Self::Whatsapp { .. } => formatter.write_str("Whatsapp([REDACTED])"),
            Self::LarkResource { .. } => formatter.write_str("LarkResource([REDACTED])"),
            Self::Discord { .. } => formatter.write_str("Discord([REDACTED])"),
            Self::NyxidRelay { index } => formatter
                .debug_struct("NyxidRelay")
                .field("index", index)
                .finish(),
            Self::Telegram {
                file_id,
                file_unique_id,
            } => formatter
                .debug_struct("Telegram")
                .field("file_id", &Redacted(file_id.len()))
                .field("file_unique_id", &Redacted(file_unique_id.len()))
                .finish(),
            Self::Url { url } => formatter
                .debug_struct("Url")
                .field("url", &Redacted(url.len()))
                .finish(),
            Self::Inline { pointer } => formatter
                .debug_struct("Inline")
                .field("pointer", &Redacted(pointer.len()))
                .finish(),
        }
    }
}

struct Redacted(usize);

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<redacted {} bytes>", self.0)
    }
}

/// The platform's own name for the media, finer than `AttachmentKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaVariant {
    Photo,
    Document,
    Audio,
    Voice,
    Video,
    VideoNote,
    Animation,
    Sticker,
    Image,
    File,
    Media,
    Embedded,
}

/// C0 and C1 controls and the bidirectional formatting characters that can
/// disguise a name, such as U+202E reversing a file extension.
pub fn forbidden_text_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// The Telegram locator grammar: `file_id` is `[A-Za-z0-9_-]{1,256}` and
/// `file_unique_id` is `[A-Za-z0-9_-]{1,128}`.
pub fn telegram_ids_valid(file_id: &str, file_unique_id: &str) -> bool {
    telegram_id(file_id, MAX_FILE_ID_BYTES) && telegram_id(file_unique_id, MAX_FILE_UNIQUE_ID_BYTES)
}

fn telegram_id(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A lowercase RFC 6838 restricted name `type/subtype` without parameters: each
/// part is 1..=127 bytes, starts with `[a-z0-9]` and continues with
/// `[a-z0-9!#$&^_.+-]`.
pub fn mime_restricted_name(s: &str) -> bool {
    s.split_once('/')
        .is_some_and(|(kind, subtype)| restricted_part(kind) && restricted_part(subtype))
}

fn restricted_part(part: &str) -> bool {
    let bytes = part.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    bytes.len() <= MAX_MIME_PART_BYTES
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && rest.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"!#$&^_.+-".contains(byte)
        })
}

/// Validate attachments against the attachment domain and the activity's
/// source, reporting pointers as `{base}/{index}/{field}`.
pub fn validate_attachments(
    attachments: &[Attachment],
    source: &SourceKind,
    base: &str,
) -> Result<(), ValidationError> {
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(ValidationError::new(base, "too many attachments"));
    }
    for (index, attachment) in attachments.iter().enumerate() {
        attachment.validate(source, &format!("{base}/{index}"))?;
    }
    Ok(())
}

impl Attachment {
    fn validate(&self, source: &SourceKind, pointer: &str) -> Result<(), ValidationError> {
        let at = |field: &str| format!("{pointer}/{field}");
        require_string(&self.reference, &at("reference"), 1, MAX_REFERENCE_BYTES)?;
        if let Some(filename) = &self.filename {
            require_text(filename, &at("filename"), MAX_FILENAME_BYTES)?;
        }
        if self
            .mime_type
            .as_deref()
            .is_some_and(|mime_type| !mime_restricted_name(mime_type))
        {
            return Err(ValidationError::new(
                at("mime_type"),
                "must be a lowercase RFC 6838 type/subtype without parameters",
            ));
        }
        if self
            .duration_seconds
            .is_some_and(|seconds| seconds > MAX_DURATION_SECONDS)
        {
            return Err(ValidationError::new(
                at("duration_seconds"),
                format!("must be at most {MAX_DURATION_SECONDS}"),
            ));
        }
        match (self.width, self.height) {
            (None, None) => {}
            (Some(_), None) => {
                return Err(ValidationError::new(at("width"), "requires height"));
            }
            (None, Some(_)) => {
                return Err(ValidationError::new(at("height"), "requires width"));
            }
            (Some(width), Some(height)) => {
                require_dimension(width, &at("width"))?;
                require_dimension(height, &at("height"))?;
            }
        }
        if let Some(emoji) = &self.emoji {
            require_text(emoji, &at("emoji"), MAX_EMOJI_BYTES)?;
        }
        if let Some(locator) = &self.source {
            validate_locator(locator, source)
                .map_err(|message| ValidationError::new(at("source"), message))?;
        }
        if let Some(thumbnail) = &self.thumbnail {
            if !matches!(
                (thumbnail, source),
                (
                    AttachmentSource::Telegram { .. },
                    SourceKind::Telegram { .. }
                ) | (
                    AttachmentSource::NativeLark {
                        resource_kind: crate::channel::LarkResourceKind::Image,
                        ..
                    },
                    SourceKind::NativeLark(_)
                )
            ) {
                return Err(ValidationError::new(
                    at("thumbnail"),
                    "must be a matching native Telegram or Lark image locator",
                ));
            }
            validate_locator(thumbnail, source)
                .map_err(|message| ValidationError::new(at("thumbnail"), message))?;
        }
        Ok(())
    }
}

/// A bounded name-like value: 1..=`max` bytes with no character that could
/// disguise it in a log line or a rendered prompt.
pub(super) fn require_text(value: &str, pointer: &str, max: usize) -> Result<(), ValidationError> {
    require_string(value, pointer, 1, max)?;
    if value.chars().any(forbidden_text_char) {
        return Err(ValidationError::new(
            pointer,
            "must not contain control or bidirectional formatting characters",
        ));
    }
    Ok(())
}

fn require_dimension(value: u32, pointer: &str) -> Result<(), ValidationError> {
    if (1..=MAX_DIMENSION).contains(&value) {
        Ok(())
    } else {
        Err(ValidationError::new(
            pointer,
            format!("must be between 1 and {MAX_DIMENSION}"),
        ))
    }
}

/// Each locator type is accepted only from the source able to resolve it, so a
/// relay activity can never direct a fetch at a URL or a Telegram file.
#[path = "attachment_locator.rs"]
mod locator;
use locator::validate_locator;

/// Whether `value` is a URL locator this domain accepts: an absolute https
/// URL of at most 2048 bytes with a host and no userinfo, fragment, whitespace
/// or controls. Shared with extractors so a URL they accept is never refused
/// by the validator. Local http is deliberately refused here even though
/// channel URLs allow it: a locator is fetched by the gateway on a sender's
/// behalf.
pub fn https_locator(value: &str) -> bool {
    if value.len() > MAX_ATTACHMENT_URL_BYTES
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

fn json_pointer(value: &str) -> bool {
    value.len() <= MAX_INLINE_POINTER_BYTES
        && value.starts_with('/')
        && value
            .split('~')
            .skip(1)
            .all(|escaped| escaped.starts_with(['0', '1']))
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "attachment_whatsapp_tests.rs"]
mod whatsapp_tests;

#[cfg(test)]
#[path = "attachment_native_lark_tests.rs"]
mod native_lark_tests;
