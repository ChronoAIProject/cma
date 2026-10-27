//! Bounded status facts from an already signature-verified Meta envelope.
use super::webhook::Error;
use cmaeg_core::channel::{
    whatsapp::{valid_group_id, valid_message_id, valid_meta_id},
    Source,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub message_id: String,
    pub recipient: String,
    pub participant: Option<String>,
    pub group: bool,
    pub kind: Kind,
    pub timestamp: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Sent,
    Delivered,
    Read,
    Played,
    Failed,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Played => "played",
            Self::Failed => "failed",
        }
    }
}

pub(crate) fn extract(source: &Source, body: &[u8]) -> Result<Vec<Status>, Error> {
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
    let mut statuses = Vec::new();
    let mut count = 0usize;
    for entry in body["entry"].as_array().ok_or(Error::Envelope)? {
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
            let Some(raw) = value.get("statuses") else {
                continue;
            };
            for item in raw.as_array().ok_or(Error::Envelope)? {
                count += 1;
                if count > super::webhook::MAX_MESSAGES {
                    return Err(Error::TooMany);
                }
                let kind = match item["status"].as_str().ok_or(Error::Message)? {
                    "sent" => Kind::Sent,
                    "delivered" => Kind::Delivered,
                    "read" => Kind::Read,
                    "played" => Kind::Played,
                    "failed" => Kind::Failed,
                    // New provider status types do not acquire a meaning by guessing.
                    _ => continue,
                };
                let message_id = item["id"]
                    .as_str()
                    .filter(|id| valid_message_id(id))
                    .ok_or(Error::Message)?;
                let group = match item.get("recipient_type") {
                    None => false,
                    Some(Value::String(kind)) if kind == "individual" => false,
                    Some(Value::String(kind)) if kind == "group" => true,
                    _ => return Err(Error::Message),
                };
                let recipient = item["recipient_id"]
                    .as_str()
                    .filter(|id| {
                        if group {
                            valid_group_id(id)
                        } else {
                            valid_meta_id(id)
                        }
                    })
                    .ok_or(Error::Message)?;
                let participant = match item.get("recipient_participant_id") {
                    Some(value) if group => Some(
                        value
                            .as_str()
                            .filter(|id| valid_meta_id(id))
                            .ok_or(Error::Message)?
                            .to_owned(),
                    ),
                    Some(_) => return Err(Error::Message),
                    None => None,
                };
                let timestamp = item["timestamp"]
                    .as_str()
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|v| *v > 0)
                    .ok_or(Error::Message)?;
                let status = Status {
                    message_id: message_id.into(),
                    recipient: recipient.into(),
                    participant,
                    group,
                    kind,
                    timestamp,
                };
                if !statuses.contains(&status) {
                    statuses.push(status);
                }
            }
        }
    }
    Ok(statuses)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
