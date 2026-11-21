//! Media facts from the authenticated native message; URLs are deliberately ignored.
use super::webhook::{hex, Content, Error};
use cmaeg_core::{
    activity::{Attachment, AttachmentKind, AttachmentSource, MediaVariant},
    channel::whatsapp::{media_sha256, valid_meta_id},
};
use serde_json::Value;

pub(super) fn extract(raw: &Value, kind: &str, message_id: &str) -> Result<Content, Error> {
    let media = raw
        .get(kind)
        .filter(|value| value.is_object())
        .ok_or(Error::Message)?;
    let id = media["id"]
        .as_str()
        .filter(|id| valid_meta_id(id))
        .ok_or(Error::Message)?;
    let hash = media["sha256"]
        .as_str()
        .and_then(media_sha256)
        .ok_or(Error::Message)?;
    let caption = match kind {
        "image" | "document" | "video" => {
            optional_text(media, "caption", 32 * 1024)?.unwrap_or_default()
        }
        _ => String::new(),
    };
    let (attachment_kind, variant) = match kind {
        "image" => (AttachmentKind::Image, MediaVariant::Image),
        "document" => (AttachmentKind::File, MediaVariant::Document),
        "audio" => {
            let voice = optional_bool(media, "voice")?.unwrap_or(false);
            (
                AttachmentKind::Audio,
                if voice {
                    MediaVariant::Voice
                } else {
                    MediaVariant::Audio
                },
            )
        }
        "video" => (AttachmentKind::Video, MediaVariant::Video),
        "sticker" => {
            optional_bool(media, "animated")?;
            (AttachmentKind::Image, MediaVariant::Sticker)
        }
        _ => return Err(Error::Message),
    };
    let mut attachment = Attachment {
        reference: format!("whatsapp:{id}"),
        kind: attachment_kind,
        filename: if kind == "document" {
            optional_text(media, "filename", 4096)?
        } else {
            None
        },
        mime_type: optional_text(media, "mime_type", 512)?,
        size_bytes: None,
        source: Some(AttachmentSource::Whatsapp {
            message_id: message_id.into(),
            media_id: id.into(),
            sha256: hex(&hash),
        }),
        variant: Some(variant),
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    };
    attachment.normalize_facts();
    Ok(Content::Media {
        caption,
        attachment: Box::new(attachment),
    })
}

fn optional_text(value: &Value, field: &str, limit: usize) -> Result<Option<String>, Error> {
    value
        .get(field)
        .map(|value| {
            value
                .as_str()
                .filter(|text| text.len() <= limit)
                .map(str::to_owned)
                .ok_or(Error::Message)
        })
        .transpose()
}
fn optional_bool(value: &Value, field: &str) -> Result<Option<bool>, Error> {
    value
        .get(field)
        .map(|value| value.as_bool().ok_or(Error::Message))
        .transpose()
}
