//! Read facts from a verified native envelope; never message/turn input.
use cmaeg_core::channel::{native_lark::safe_id, NativeLarkIdentity};
use serde_json::Value;

pub(crate) const EVENT_TYPE: &str = "im.message.message_read_v1";
/// Local work bound, independent of the provider's batch limit.
const MAX_MESSAGES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadStatus {
    pub reader: String,
    pub occurred_ms: i64,
    pub message_ids: Vec<String>,
}

pub(crate) fn extract(
    identity: &NativeLarkIdentity,
    event: &Value,
    now: i64,
) -> Result<ReadStatus, ()> {
    let header = &event["header"];
    let reader = &event["event"]["reader"];
    if event["schema"] != "2.0"
        || header["event_type"] != EVENT_TYPE
        || header["app_id"] != identity.app_id
        || header["tenant_key"] != identity.tenant_key
        || !header["event_id"].as_str().is_some_and(safe_id)
        || reader["tenant_key"] != identity.tenant_key
    {
        return Err(());
    }
    let actor = reader["reader_id"]["open_id"]
        .as_str()
        .filter(|id| safe_id(id) && id.starts_with("ou_") && *id != identity.bot_open_id)
        .ok_or(())?;
    let timestamp = reader["read_time"].as_str().ok_or(())?;
    let occurred_ms = timestamp.parse::<i64>().map_err(|_| ())?;
    if occurred_ms.to_string() != timestamp
        || occurred_ms <= 0
        || occurred_ms / 1000 > now.saturating_add(300)
        || occurred_ms / 1000 <= now.saturating_sub(super::receipts::RETENTION)
    {
        return Err(());
    }
    let messages = event["event"]["message_id_list"]
        .as_array()
        .filter(|ids| !ids.is_empty() && ids.len() <= MAX_MESSAGES)
        .ok_or(())?;
    let mut message_ids = Vec::new();
    // Validate the entire batch before any observation; duplicates are one fact.
    for message in messages {
        let id = message
            .as_str()
            .filter(|id| safe_id(id) && id.starts_with("om_"))
            .ok_or(())?;
        if !message_ids.iter().any(|existing| existing == id) {
            message_ids.push(id.to_owned());
        }
    }
    Ok(ReadStatus {
        reader: actor.into(),
        occurred_ms,
        message_ids,
    })
}

#[cfg(test)]
#[path = "read_status_tests.rs"]
mod tests;
