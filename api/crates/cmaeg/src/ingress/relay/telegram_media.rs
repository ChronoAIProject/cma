//! Telegram media turned into activity attachments.
//!
//! The NyxID relay and the native webhook deliver the same authenticated
//! Telegram `Message`. Its media fields become attachments in NyxID's order,
//! each with a typed locator and normalized facts. Extraction never refuses: a
//! field of an unexpected shape is skipped, an invalid locator leaves the
//! attachment not retrievable, and every sender-declared fact passes through
//! [`Attachment::normalize_facts`]. It does no network I/O; bytes are fetched
//! later through the locator.
use super::attachments::RelayAttachments;
use crate::telemetry::names;
use cmaeg_core::activity::{
    forbidden_text_char, telegram_ids_valid, Attachment, AttachmentKind, AttachmentSource,
    MediaVariant, MAX_ALBUM_ID_BYTES,
};
use serde_json::Value;

/// Whether, and through which transport, an observer admits Telegram media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaIngress {
    Off,
    Relay,
    Native,
}

impl MediaIngress {
    pub(crate) fn relay(enabled: bool) -> Self {
        if enabled {
            Self::Relay
        } else {
            Self::Off
        }
    }

    pub(crate) fn native(enabled: bool) -> Self {
        if enabled {
            Self::Native
        } else {
            Self::Off
        }
    }
}

/// Where the extracted attachments' bytes can later be fetched from.
#[derive(Clone, Copy)]
pub(crate) enum Transport<'a> {
    /// Through NyxID, by the position of the envelope item listing the file.
    Relay(&'a RelayAttachments),
    /// Through the Bot API, by the message's own file identifiers.
    Native,
}

/// The media of one Telegram message.
pub(crate) struct TelegramMedia {
    pub attachments: Vec<Attachment>,
    pub album_id: Option<String>,
    pub caption: Option<String>,
    /// The `content.type` NyxID derives for this message.
    pub nyxid_type: &'static str,
}

/// Media fields in the order NyxID lists their attachments.
pub(crate) const MEDIA_FIELDS: [&str; 8] = [
    "photo",
    "document",
    "audio",
    "video",
    "voice",
    "sticker",
    "video_note",
    "animation",
];
/// The largest photo the native path prefers: a long side the image pipeline
/// keeps without scaling, and a size the native download comfortably fetches.
const MAX_NATIVE_PHOTO_SIDE: u64 = 2048;
const MAX_NATIVE_PHOTO_BYTES: u64 = 8 * 1024 * 1024;

/// Extract the media of `message`, or `None` when it has no media field.
///
/// `source` is the bounded metric label of the transport (`nyxid_relay` or
/// `telegram`). Identifiers, names and captions never reach logs or labels.
pub(crate) fn extract(
    message: &Value,
    transport: Transport<'_>,
    source: &'static str,
) -> Option<TelegramMedia> {
    let nyxid_type = media_content_type(message)?;
    // An animation message also lists its file as a document with the same file_id.
    let animation_id = message
        .get("animation")
        .and_then(|animation| animation.get("file_id"))
        .and_then(Value::as_str);
    let mut used = match transport {
        Transport::Relay(list) => vec![false; list.kept.len()],
        Transport::Native => Vec::new(),
    };
    let mut attachments = Vec::new();
    for field in MEDIA_FIELDS {
        let Some(value) = message.get(field) else {
            continue;
        };
        let Some(raw) = media_object(field, value, transport) else {
            tracing::debug!(field, "telegram media field skipped");
            continue;
        };
        if field == "document" && animation_id.is_some() && file_id(raw) == animation_id {
            continue;
        }
        let mut attachment = build(field, raw, source);
        locate(
            &mut attachment,
            raw,
            attachments.len(),
            transport,
            &mut used,
            source,
        );
        metrics::counter!(
            names::MEDIA_ITEMS,
            names::SOURCE => source,
            names::KIND => kind_label(attachment.kind),
            names::OUTCOME => "extracted"
        )
        .increment(1);
        attachments.push(attachment);
    }
    let album_id = album(message);
    tracing::debug!(
        source,
        attachments = attachments.len(),
        album = album_id.is_some(),
        "telegram media extracted"
    );
    Some(TelegramMedia {
        attachments,
        album_id,
        caption: message
            .get("caption")
            .and_then(Value::as_str)
            .map(str::to_owned),
        nyxid_type,
    })
}

/// The object an attachment is built from: the chosen size of a photo, or the
/// field's own object.
fn media_object<'a>(field: &str, value: &'a Value, transport: Transport<'_>) -> Option<&'a Value> {
    if field != "photo" {
        return value.is_object().then_some(value);
    }
    let sizes = value.as_array()?;
    match transport {
        // The last size is the one NyxID lists, so correlation finds it.
        Transport::Relay(_) => sizes.last().filter(|size| size.is_object()),
        Transport::Native => {
            let objects = || sizes.iter().filter(|size| size.is_object());
            objects()
                .filter(|size| fits_native(size))
                .max_by_key(|size| area(size))
                .or_else(|| objects().min_by_key(|size| area(size)))
        }
    }
}

fn side(size: &Value, key: &str) -> Option<u64> {
    size.get(key).and_then(Value::as_u64)
}

fn area(size: &Value) -> u64 {
    side(size, "width")
        .unwrap_or_default()
        .saturating_mul(side(size, "height").unwrap_or_default())
}

/// A size without a declared `file_size` is not excluded: the download enforces
/// its own byte cap.
fn fits_native(size: &Value) -> bool {
    let fits_side = matches!(
        (side(size, "width"), side(size, "height")),
        (Some(width), Some(height)) if width.max(height) <= MAX_NATIVE_PHOTO_SIDE
    );
    fits_side && side(size, "file_size").is_none_or(|bytes| bytes <= MAX_NATIVE_PHOTO_BYTES)
}

fn file_id(raw: &Value) -> Option<&str> {
    raw.get("file_id").and_then(Value::as_str)
}

fn text(raw: &Value, key: &str) -> Option<String> {
    raw.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// A declared number, absent when it does not fit the fact's type.
fn number(raw: &Value, key: &str) -> Option<u32> {
    raw.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// Build the attachment's facts for `field`, normalized into the validated domain.
fn build(field: &str, raw: &Value, source: &'static str) -> Attachment {
    let mut attachment = Attachment {
        reference: String::new(),
        kind: AttachmentKind::File,
        filename: None,
        mime_type: None,
        size_bytes: raw.get("file_size").and_then(Value::as_u64),
        source: None,
        variant: None,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    };
    let (kind, variant) = match field {
        "photo" => {
            // Telegram re-encodes every photo size as JPEG.
            attachment.mime_type = Some("image/jpeg".into());
            (attachment.width, attachment.height) = (number(raw, "width"), number(raw, "height"));
            (AttachmentKind::Image, MediaVariant::Photo)
        }
        "document" => {
            attachment.filename = text(raw, "file_name");
            attachment.mime_type = text(raw, "mime_type");
            (AttachmentKind::File, MediaVariant::Document)
        }
        "audio" => {
            attachment.duration_seconds = number(raw, "duration");
            attachment.filename = text(raw, "file_name");
            attachment.mime_type = text(raw, "mime_type");
            (AttachmentKind::Audio, MediaVariant::Audio)
        }
        "voice" => {
            attachment.duration_seconds = number(raw, "duration");
            attachment.mime_type = text(raw, "mime_type");
            (AttachmentKind::Audio, MediaVariant::Voice)
        }
        "video" | "animation" => {
            attachment.duration_seconds = number(raw, "duration");
            (attachment.width, attachment.height) = (number(raw, "width"), number(raw, "height"));
            attachment.filename = text(raw, "file_name");
            attachment.mime_type = text(raw, "mime_type");
            let variant = if field == "video" {
                MediaVariant::Video
            } else {
                MediaVariant::Animation
            };
            (AttachmentKind::Video, variant)
        }
        "video_note" => {
            // A round video: its one declared length is both width and height.
            let length = number(raw, "length");
            (attachment.width, attachment.height) = (length, length);
            attachment.duration_seconds = number(raw, "duration");
            (AttachmentKind::Video, MediaVariant::VideoNote)
        }
        _ => {
            attachment.emoji = text(raw, "emoji");
            (attachment.width, attachment.height) = (number(raw, "width"), number(raw, "height"));
            let flag = |key| raw.get(key).and_then(Value::as_bool) == Some(true);
            let kind = if flag("is_video") {
                AttachmentKind::Video
            } else if flag("is_animated") {
                AttachmentKind::Other
            } else {
                AttachmentKind::Image
            };
            (kind, MediaVariant::Sticker)
        }
    };
    (attachment.kind, attachment.variant) = (kind, Some(variant));
    for repaired in attachment.normalize_facts().fields() {
        metrics::counter!(names::MEDIA_FACTS_REPAIRED, names::SOURCE => source, names::FIELD => repaired)
            .increment(1);
    }
    if variant == MediaVariant::Document {
        attachment.kind = match attachment.mime_type.as_deref() {
            Some(mime) if mime.starts_with("image/") => AttachmentKind::Image,
            Some(mime) if mime.starts_with("audio/") => AttachmentKind::Audio,
            Some(mime) if mime.starts_with("video/") => AttachmentKind::Video,
            _ => AttachmentKind::File,
        };
    }
    attachment
}

/// Set the reference and the locator. Invalid identifiers leave the attachment
/// not retrievable rather than refusing the message.
fn locate(
    attachment: &mut Attachment,
    raw: &Value,
    position: usize,
    transport: Transport<'_>,
    used: &mut [bool],
    source: &'static str,
) {
    let file_id = file_id(raw).unwrap_or_default();
    let unique_id = raw
        .get("file_unique_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let valid = telegram_ids_valid(file_id, unique_id);
    attachment.reference = if valid {
        format!("telegram:{unique_id}")
    } else {
        format!("telegram:item-{position}")
    };
    match transport {
        Transport::Relay(list) => {
            attachment.source = correlate(list, used, file_id);
            let outcome = if attachment.source.is_some() {
                "matched"
            } else {
                "unmatched"
            };
            metrics::counter!(names::MEDIA_RELAY_CORRELATION, names::OUTCOME => outcome)
                .increment(1);
        }
        Transport::Native if valid => {
            attachment.source = Some(AttachmentSource::Telegram {
                file_id: file_id.to_owned(),
                file_unique_id: unique_id.to_owned(),
            });
            attachment.thumbnail = raw.get("thumbnail").and_then(telegram_locator);
        }
        Transport::Native => {
            metrics::counter!(
                names::MEDIA_ITEMS,
                names::SOURCE => source,
                names::KIND => kind_label(attachment.kind),
                names::OUTCOME => "invalid_locator"
            )
            .increment(1);
        }
    }
}

/// The first unused envelope item, in envelope order, whose opaque `url` is
/// this file's `file_id`; its original position is NyxID's download index.
fn correlate(
    list: &RelayAttachments,
    used: &mut [bool],
    file_id: &str,
) -> Option<AttachmentSource> {
    if file_id.is_empty() {
        return None;
    }
    let position = list
        .kept
        .iter()
        .zip(used.iter())
        .position(|(item, used)| !used && item.attachment.url == file_id)?;
    used[position] = true;
    Some(AttachmentSource::NyxidRelay {
        index: list.kept[position].index,
    })
}

fn telegram_locator(size: &Value) -> Option<AttachmentSource> {
    let file_id = file_id(size)?;
    let unique_id = size.get("file_unique_id").and_then(Value::as_str)?;
    telegram_ids_valid(file_id, unique_id).then(|| AttachmentSource::Telegram {
        file_id: file_id.to_owned(),
        file_unique_id: unique_id.to_owned(),
    })
}

fn album(message: &Value) -> Option<String> {
    message
        .get("media_group_id")
        .and_then(Value::as_str)
        .filter(|id| {
            (1..=MAX_ALBUM_ID_BYTES).contains(&id.len()) && !id.chars().any(forbidden_text_char)
        })
        .map(str::to_owned)
}

/// NyxID's `content.type` precedence for a media message, or `None` when the
/// message has no media field. Native ingress builds the same envelope type.
pub(crate) fn media_content_type(message: &Value) -> Option<&'static str> {
    let has = |field| message.get(field).is_some();
    if has("photo") {
        Some("image")
    } else if has("document") {
        Some("file")
    } else if has("audio") {
        Some("audio")
    } else if has("video") || has("video_note") || has("animation") {
        Some("video")
    } else if has("voice") {
        Some("audio")
    } else if has("sticker") {
        Some("image")
    } else {
        None
    }
}

fn kind_label(kind: AttachmentKind) -> &'static str {
    match kind {
        AttachmentKind::Image => "image",
        AttachmentKind::File => "file",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Video => "video",
        AttachmentKind::Other => "other",
    }
}

#[cfg(test)]
#[path = "telegram_media_tests.rs"]
mod tests;
