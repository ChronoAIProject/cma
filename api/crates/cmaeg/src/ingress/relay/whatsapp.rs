//! Opt-in WhatsApp Cloud API observer. NyxID relays each raw message separately.
use super::{
    facts::{CallbackFacts, Observation, RelayFacts},
    payload::{identifier, CallbackPayload},
    verified_media::{self, numeric_id, Identity},
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, AttachmentKind, ConversationKind, MediaVariant};
use serde_json::Value;

const INVALID: VerificationError = VerificationError("whatsapp_event_invalid");

pub(super) fn observe(
    payload: &CallbackPayload,
    media_enabled: bool,
) -> Result<Observation, VerificationError> {
    let Some(raw) = payload
        .raw_platform_data
        .as_ref()
        .filter(|raw| raw.is_object())
    else {
        return Ok(Observation::Refused(Refusal::UnknownSender));
    };
    let Some(kind) = raw["type"].as_str() else {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    };
    if !matches!(
        kind,
        "text"
            | "image"
            | "audio"
            | "video"
            | "document"
            | "sticker"
            | "location"
            | "contacts"
            | "button"
            | "interactive"
    ) {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    }
    let sender = numeric_id(&raw["from"]).ok_or(INVALID)?;
    let message_id = raw["id"]
        .as_str()
        .filter(|id| identifier(id))
        .ok_or(INVALID)?;
    let parent = raw["context"]["id"].as_str();
    if sender != payload.sender.platform_id
        || sender != payload.conversation.platform_id
        || payload.conversation.kind != "private"
        || parent != payload.reply_to_platform_message_id.as_deref()
    {
        return Err(VerificationError("whatsapp_identity_mismatch"));
    }
    let media_kind = match kind {
        "image" | "sticker" => Some((AttachmentKind::Image, "image")),
        "audio" => Some((AttachmentKind::Audio, "audio")),
        "video" => Some((AttachmentKind::Video, "video")),
        "document" => Some((AttachmentKind::File, "file")),
        _ => None,
    };
    if !media_enabled && (media_kind.is_some() || payload.received_attachments() > 0) {
        return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
    }
    let text = text(raw, kind);
    if text != payload.content.text
        || text.as_ref().is_some_and(|text| text.len() > 32 * 1024)
        || payload.content.kind != media_kind.map_or("text", |(_, label)| label)
    {
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
    let attachments = if let Some((kind, _)) = media_kind {
        let media = &raw[raw["type"].as_str().unwrap()];
        let mut attachment = verified_media::extract(
            Identity {
                platform: "whatsapp",
                message_id,
                file_id: numeric_id(&media["id"]),
                url: None,
            },
            media,
            kind,
            &payload.content.attachments,
            0,
        );
        if raw["type"] == "sticker" {
            attachment.variant = Some(MediaVariant::Sticker);
        } else if kind == AttachmentKind::Audio && media["voice"] == true {
            attachment.variant = Some(MediaVariant::Voice);
        }
        vec![attachment]
    } else {
        if payload.received_attachments() > 0 {
            return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
        }
        if text.as_ref().is_none_or(String::is_empty) {
            return Ok(Observation::Refused(Refusal::ContentUnsupported));
        }
        Vec::new()
    };
    let callback = callback(raw, payload, message_id, parent)?;
    Ok(Observation::Message(RelayFacts {
        thread: None,
        callback: callback.map(Box::new),
        event: None,
        conversation: ConversationKind::Private,
        thread_id: None,
        mentioned: Some(false),
        replied_to_bot: if parent.is_some() { None } else { Some(false) },
        // Cloud API identifies the sending account, not whether a person or
        // automation controls it. Do not fabricate a human/bot distinction.
        actor: ActorKind::Unknown,
        text,
        attachments,
        album_id: None,
    }))
}

/// Only our issued locators become candidates for durable authorization. Foreign
/// controls remain ordinary text; callback titles never supply action values.
fn callback(
    raw: &Value,
    payload: &CallbackPayload,
    message_id: &str,
    parent: Option<&str>,
) -> Result<Option<CallbackFacts>, VerificationError> {
    if raw["type"] != "interactive" {
        return Ok(None);
    }
    let interactive = &raw["interactive"];
    let selection = interactive
        .get("button_reply")
        .or_else(|| interactive.get("list_reply"));
    let Some(data) = selection
        .and_then(|value| value["id"].as_str())
        .filter(|s| s.starts_with("cw1b:") || s.starts_with("cw1l:"))
    else {
        return Ok(None);
    };
    let expected = if data.starts_with("cw1b:") {
        "button_reply"
    } else {
        "list_reply"
    };
    if data.len() > 64
        || interactive["type"] != expected
        || interactive.get(expected).is_none()
        || (interactive.get("button_reply").is_some() && interactive.get("list_reply").is_some())
        || payload.thread_id.is_some()
    {
        return Err(INVALID);
    }
    let parent = parent.filter(|id| identifier(id)).ok_or(INVALID)?;
    Ok(Some(CallbackFacts {
        id: message_id.into(),
        data: data.into(),
        message_id: parent.into(),
        form: None,
    }))
}

/// Match NyxID's text normalization, independently of callback authority.
fn text(message: &Value, kind: &str) -> Option<String> {
    let value = &message[kind];
    match kind {
        "text" => value["body"].as_str().map(str::to_owned),
        "image" | "video" | "audio" | "document" | "sticker" => {
            value["caption"].as_str().map(str::to_owned)
        }
        "button" => value["text"]
            .as_str()
            .or_else(|| value["payload"].as_str())
            .map(str::to_owned),
        "interactive" => {
            let choice = value
                .get("button_reply")
                .or_else(|| value.get("list_reply"))?;
            choice["title"]
                .as_str()
                .or_else(|| choice["id"].as_str())
                .map(str::to_owned)
        }
        "location" => {
            let mut text = format!(
                "Location: {}, {}",
                value["latitude"].as_f64()?,
                value["longitude"].as_f64()?
            );
            for key in ["name", "address"] {
                if let Some(detail) = value[key].as_str() {
                    text.push_str(&format!("; {detail}"));
                }
            }
            Some(text)
        }
        "contacts" => Some(format!(
            "Contacts: {}",
            value
                .as_array()?
                .iter()
                .filter_map(|contact| contact["name"]["formatted_name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => None,
    }
}
