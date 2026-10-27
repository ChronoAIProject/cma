//! Facts extracted only after authenticating Meta's original request bytes.
use cmaeg_core::{
    channel::{
        whatsapp::{valid_message_id, valid_meta_id},
        Source,
    },
    ids::EventId,
};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_MESSAGES: usize = 100;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Signature,
    Envelope,
    Message,
    TooMany,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Text {
        text: String,
    },
    Media {
        caption: String,
        attachment: Box<cmaeg_core::activity::Attachment>,
    },
    Reaction {
        target: String,
        emoji: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub id: String,
    pub sender: String,
    pub timestamp: i64,
    pub content: Content,
}

pub fn verify(secret: &str, signature: &str, body: &[u8]) -> Result<(), Error> {
    let encoded = signature
        .strip_prefix("sha256=")
        .filter(|value| value.len() == 64)
        .ok_or(Error::Signature)?;
    let mut expected = [0u8; 32];
    for (output, pair) in expected.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        *output = digit(pair[0]).ok_or(Error::Signature)? * 16
            + digit(pair[1]).ok_or(Error::Signature)?;
    }
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes()),
        body,
        &expected,
    )
    .map_err(|_| Error::Signature)
}

/// Signature authenticates the configured app's secret. Account and phone are
/// checked independently; the payload never supplies authoritative app identity.
pub fn extract(source: &Source, body: &[u8]) -> Result<Vec<Message>, Error> {
    let Source::Whatsapp {
        waba_id,
        phone_number_id,
        ..
    } = source
    else {
        return Err(Error::Envelope);
    };
    let body: Value = serde_json::from_slice(body).map_err(|_| Error::Envelope)?;
    if body["object"] != "whatsapp_business_account" {
        return Err(Error::Envelope);
    }
    let entries = body["entry"].as_array().ok_or(Error::Envelope)?;
    let mut messages = Vec::new();
    let mut seen = std::collections::BTreeMap::new();
    for entry in entries {
        if entry["id"].as_str() != Some(waba_id) {
            continue;
        }
        for change in entry["changes"].as_array().ok_or(Error::Envelope)? {
            let value = &change["value"];
            if change["field"] != "messages"
                || value["messaging_product"] != "whatsapp"
                || value["metadata"]["phone_number_id"].as_str() != Some(phone_number_id)
            {
                continue;
            }
            if !private_shape(value) {
                return Err(Error::Message);
            }
            // Status-only deliveries are acknowledged; they never establish a
            // message or reaction target and never start provider work.
            let Some(raw_messages) = value.get("messages") else {
                continue;
            };
            for raw in raw_messages.as_array().ok_or(Error::Envelope)? {
                if let Some(message) = parse(raw)? {
                    if let Some(prior) = seen.get(&message.id) {
                        if prior != &message {
                            return Err(Error::Message);
                        }
                        continue;
                    }
                    seen.insert(message.id.clone(), message.clone());
                    messages.push(message);
                }
                if messages.len() > MAX_MESSAGES {
                    return Err(Error::TooMany);
                }
            }
        }
    }
    Ok(messages)
}

fn parse(raw: &Value) -> Result<Option<Message>, Error> {
    let kind = raw["type"].as_str().ok_or(Error::Message)?;
    if !matches!(
        kind,
        "text" | "reaction" | "image" | "document" | "audio" | "video" | "sticker"
    ) {
        return Ok(None);
    }
    if !private_shape(raw) {
        return Err(Error::Message);
    }
    let id = raw["id"]
        .as_str()
        .filter(|id| valid_message_id(id))
        .ok_or(Error::Message)?;
    let sender = raw["from"]
        .as_str()
        .filter(|id| valid_meta_id(id))
        .ok_or(Error::Message)?;
    let timestamp = raw["timestamp"]
        .as_str()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or(Error::Message)?;
    let content = if kind == "text" {
        let text = raw["text"]["body"]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= 32 * 1024)
            .ok_or(Error::Message)?;
        Content::Text {
            text: text.to_owned(),
        }
    } else if kind == "reaction" {
        let target = raw["reaction"]["message_id"]
            .as_str()
            .filter(|id| valid_message_id(id))
            .ok_or(Error::Message)?;
        // Meta sends an empty string when removing the sender's reaction. It
        // does not report the previous emoji, so no old-state value is inferred.
        let emoji = raw["reaction"]["emoji"]
            .as_str()
            .filter(|emoji| emoji.len() <= 512 && !emoji.chars().any(char::is_control))
            .ok_or(Error::Message)?;
        Content::Reaction {
            target: target.to_owned(),
            emoji: emoji.to_owned(),
        }
    } else {
        super::media::extract(raw, kind, id)?
    };
    Ok(Some(Message {
        id: id.to_owned(),
        sender: sender.to_owned(),
        timestamp,
        content,
    }))
}

fn private_shape(value: &Value) -> bool {
    ["group_id", "group", "participant"]
        .iter()
        .all(|field| value.get(field).is_none())
        && value
            .get("recipient_type")
            .is_none_or(|kind| kind == "individual")
}

pub fn event_id(source: &Source, message_id: &str) -> Result<EventId, Error> {
    let Source::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = source
    else {
        return Err(Error::Envelope);
    };
    let bytes = serde_json::to_vec(&(app_id, waba_id, phone_number_id, message_id))
        .map_err(|_| Error::Message)?;
    EventId::new(format!("whatsapp:{}", hex(&Sha256::digest(bytes)))).map_err(|_| Error::Message)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
