//! Coexistence callbacks change visible text only. Payload URLs never grant media access.
use super::webhook::{private_shape, Content, Error};
use cmaeg_core::channel::whatsapp::valid_message_id;
use serde_json::Value;

pub(super) fn extract(raw: &Value, kind: &str, id: &str) -> Result<Content, Error> {
    for field in [
        "text",
        "image",
        "video",
        "document",
        "audio",
        "sticker",
        "reaction",
        "interactive",
    ] {
        if raw.get(field).is_some() {
            return Err(Error::Message);
        }
    }
    let change = raw
        .get(kind)
        .filter(|value| value.is_object())
        .ok_or(Error::Message)?;
    let target = change["original_message_id"]
        .as_str()
        .filter(|target| valid_message_id(target) && *target != id)
        .ok_or(Error::Message)?;
    if kind == "revoke" {
        if raw.get("edit").is_some() || change.get("message").is_some() {
            return Err(Error::Message);
        }
        return Ok(Content::Revoke {
            target: target.into(),
        });
    }
    if raw.get("revoke").is_some() {
        return Err(Error::Message);
    }
    let message = change
        .get("message")
        .filter(|value| value.is_object())
        .ok_or(Error::Message)?;
    if !private_shape(message)
        || message
            .get("from")
            .is_some_and(|sender| sender != &raw["from"])
        || message
            .get("id")
            .is_some_and(|id| id.as_str() != Some(target))
    {
        return Err(Error::Message);
    }
    let kind = message["type"].as_str().ok_or(Error::Message)?;
    // Reject ambiguous content or a media-kind substitution rather than guessing.
    for field in [
        "text",
        "image",
        "video",
        "document",
        "audio",
        "sticker",
        "reaction",
        "interactive",
        "edit",
        "revoke",
    ] {
        if field != kind && message.get(field).is_some() {
            return Err(Error::Message);
        }
    }
    let (text, media) = match kind {
        "text" => {
            let body = message["text"]["body"]
                .as_str()
                .filter(|body| !body.is_empty() && body.len() <= 32 * 1024)
                .ok_or(Error::Message)?;
            (body.into(), None)
        }
        "image" | "video" | "document" => {
            if !message[kind]["caption"].is_string() {
                return Err(Error::Message);
            }
            let Content::Media {
                caption,
                attachment,
            } = super::media::extract(message, kind, target)?
            else {
                return Err(Error::Message);
            };
            (caption, Some(attachment))
        }
        _ => return Err(Error::Message),
    };
    Ok(Content::Edit {
        target: target.into(),
        text,
        media,
    })
}

pub(crate) fn matches_original(change: &Content, original: &Content) -> bool {
    match (change, original) {
        (Content::Revoke { .. }, Content::Text { .. } | Content::Media { .. }) => true,
        (Content::Edit { media: None, .. }, Content::Text { .. }) => true,
        (
            Content::Edit {
                media: Some(edited),
                ..
            },
            Content::Media { attachment, .. },
        ) => {
            // Exact retained media identity/type; only the caption becomes a patch.
            edited.source == attachment.source
                && edited.variant == attachment.variant
                && edited.kind == attachment.kind
        }
        _ => false,
    }
}
