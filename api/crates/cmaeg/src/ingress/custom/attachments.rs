//! Attachment items a custom event lists in its data, under the owner's setting.
//!
//! Custom senders get a synchronous answer, so items are validated strictly
//! and a failure reports the exact JSON pointer instead of being silently
//! repaired. Relay and native senders cannot be told about a refusal, so their
//! facts are normalized instead. Nothing here fetches: a URL or an inline
//! payload only becomes a locator that later retrieval resolves.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{
        https_locator, mime_restricted_name, validate_attachments, Attachment, AttachmentKind,
        AttachmentSource, SourceKind, MAX_ATTACHMENTS,
    },
    channel::CustomAttachments,
    cloudevent::CloudEvent,
};
use serde_json::{Map, Value};
use std::borrow::Cow;

/// Decoded bytes one inline `data:` item may carry. The 256 KiB event body
/// bounds the whole event as well.
pub const MAX_INLINE_ATTACHMENT_BYTES: usize = 192 * 1_024;
/// The only keys an item may carry; anything else is refused at its pointer.
const ITEM_KEYS: [&str; 8] = [
    "url",
    "kind",
    "mime_type",
    "filename",
    "size_bytes",
    "duration_seconds",
    "width",
    "height",
];

/// The first invalid value, as a JSON pointer relative to the event's `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentError {
    pub pointer: String,
}

impl AttachmentError {
    fn at(pointer: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
        }
    }
}

/// Build the attachments listed at the configured pointer of `ce.data`.
///
/// An absent or null target lists nothing. Every built item is then checked by
/// the same validator that guards stored activities, whose pointer is returned
/// unchanged.
pub fn extract(
    ce: &CloudEvent,
    config: &CustomAttachments,
) -> Result<Vec<Attachment>, AttachmentError> {
    let pointer = config.pointer.as_str();
    let target = ce.data.as_ref().and_then(|data| data.pointer(pointer));
    let items = match target {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) if items.len() <= MAX_ATTACHMENTS => items,
        Some(_) => return Err(AttachmentError::at(pointer)),
    };
    let built = items
        .iter()
        .enumerate()
        .map(|(index, item)| build(item, index, pointer, &config.origins))
        .collect::<Result<Vec<_>, _>>()?;
    validate_attachments(&built, &SourceKind::Custom, pointer)
        .map_err(|error| AttachmentError::at(error.pointer))?;
    Ok(built)
}

/// The event as prompts render it: the configured pointer's target is replaced
/// by a short marker, so neither inline base64 nor item URLs reach a prompt.
pub fn elided<'a>(ce: &'a CloudEvent, pointer: &str, count: usize) -> Cow<'a, CloudEvent> {
    if count == 0 {
        return Cow::Borrowed(ce);
    }
    let mut event = ce.clone();
    if let Some(target) = event
        .data
        .as_mut()
        .and_then(|data| data.pointer_mut(pointer))
    {
        *target = Value::String(format!("[{count} attachments, listed below]"));
    }
    Cow::Owned(event)
}

fn build(
    item: &Value,
    index: usize,
    pointer: &str,
    origins: &[String],
) -> Result<Attachment, AttachmentError> {
    let base = format!("{pointer}/{index}");
    let Some(fields) = item.as_object() else {
        return Err(AttachmentError::at(base));
    };
    if let Some(key) = fields.keys().find(|key| !ITEM_KEYS.contains(&key.as_str())) {
        return Err(AttachmentError::at(format!("{base}/{}", escape(key))));
    }
    let at = |key: &str| AttachmentError::at(format!("{base}/{key}"));
    let Some(Value::String(url)) = fields.get("url") else {
        return Err(at("url"));
    };
    let kind = match fields.get("kind") {
        None => None,
        Some(value) => {
            Some(serde_json::from_value::<AttachmentKind>(value.clone()).map_err(|_| at("kind"))?)
        }
    };
    let mut mime_type = text(fields, "mime_type").map_err(|_| at("mime_type"))?;
    let filename = text(fields, "filename").map_err(|_| at("filename"))?;
    let mut size_bytes = number(fields, "size_bytes").map_err(|_| at("size_bytes"))?;
    let duration_seconds = small(fields, "duration_seconds").map_err(|_| at("duration_seconds"))?;
    let width = small(fields, "width").map_err(|_| at("width"))?;
    let height = small(fields, "height").map_err(|_| at("height"))?;
    let source = if let Some(uri) = url.strip_prefix("data:") {
        let inline = data_uri(uri).ok_or_else(|| at("url"))?;
        if size_bytes.is_some_and(|declared| declared != inline.decoded_bytes) {
            return Err(at("size_bytes"));
        }
        size_bytes = Some(inline.decoded_bytes);
        mime_type = mime_type.or(Some(inline.mime_type));
        AttachmentSource::Inline {
            pointer: format!("{base}/url"),
        }
    } else if allowed_url(url, origins) {
        AttachmentSource::Url { url: url.clone() }
    } else {
        return Err(at("url"));
    };
    Ok(Attachment {
        reference: format!("custom:{index}"),
        kind: kind.unwrap_or_else(|| kind_of(mime_type.as_deref())),
        filename,
        mime_type,
        size_bytes,
        source: Some(source),
        variant: None,
        duration_seconds,
        width,
        height,
        thumbnail: None,
        emoji: None,
    })
}

/// An optional fact of the wrong JSON type, `null` included, is an error:
/// facts are copied verbatim, never coerced.
fn text(fields: &Map<String, Value>, key: &str) -> Result<Option<String>, ()> {
    match fields.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

fn number(fields: &Map<String, Value>, key: &str) -> Result<Option<u64>, ()> {
    fields
        .get(key)
        .map(|value| value.as_u64().ok_or(()))
        .transpose()
}

fn small(fields: &Map<String, Value>, key: &str) -> Result<Option<u32>, ()> {
    number(fields, key)?
        .map(|value| u32::try_from(value).map_err(|_| ()))
        .transpose()
}

fn kind_of(mime_type: Option<&str>) -> AttachmentKind {
    match mime_type.and_then(|value| value.split_once('/')) {
        Some(("image", _)) => AttachmentKind::Image,
        Some(("audio", _)) => AttachmentKind::Audio,
        Some(("video", _)) => AttachmentKind::Video,
        _ => AttachmentKind::File,
    }
}

/// The URL must satisfy the locator rules the activity validator applies, so
/// a URL accepted here is never refused later at an unrelated pointer.
fn allowed_url(value: &str, origins: &[String]) -> bool {
    https_locator(value)
        && url::Url::parse(value).is_ok_and(|url| {
            let origin = url.origin().ascii_serialization();
            origins.contains(&origin)
        })
}

#[derive(Debug, PartialEq, Eq)]
struct InlineData {
    mime_type: String,
    decoded_bytes: u64,
}

/// Parse the part after `data:`, exactly `<type>/<subtype>[;<param>]*;base64,<payload>`.
fn data_uri(uri: &str) -> Option<InlineData> {
    let (header, payload) = uri.split_once(',')?;
    let mut parts = header.strip_suffix(";base64")?.split(';');
    let mime_type = parts.next()?.to_ascii_lowercase();
    if !mime_restricted_name(&mime_type) || !parts.all(parameter) {
        return None;
    }
    let decoded = decoded_length(payload)?;
    if decoded > MAX_INLINE_ATTACHMENT_BYTES || STANDARD.decode(payload).is_err() {
        return None;
    }
    Some(InlineData {
        mime_type,
        decoded_bytes: u64::try_from(decoded).ok()?,
    })
}

/// An RFC 2397 `attribute=value` parameter such as `charset=utf-8`.
fn parameter(part: &str) -> bool {
    part.split_once('=').is_some_and(|(name, value)| {
        !name.is_empty()
            && !value.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\\'))
    })
}

/// The exact decoded size of padded standard base64, computed before decoding
/// so an oversized payload is refused without allocating for it.
fn decoded_length(payload: &str) -> Option<usize> {
    let bytes = payload.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return None;
    }
    Some(bytes.len() / 4 * 3 - padding)
}

/// RFC 6901 reference token escaping, so an unknown key's pointer is exact.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
