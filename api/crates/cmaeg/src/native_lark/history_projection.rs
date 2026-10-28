//! One bounded raw page. Provider pagination never creates message/file authority.
use super::Error;
use cmaeg_core::{
    activity::{Actor, ActorKind},
    channel::native_lark::safe_id,
    conversation_history::{
        HistoryMessage, HistoryOmittedContent, HISTORY_PAGE_SIZE, HISTORY_TEXT_BYTES,
    },
    seal::NativeLarkHistoryCursorPayload,
};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) struct Projection {
    pub messages: Vec<HistoryMessage>,
    pub scanned: u8,
    pub omitted: u8,
    pub next: Option<String>,
    pub before: String,
    pub last_ms: Option<i64>,
    pub last_id: Option<String>,
    pub text_truncated: bool,
}
pub(crate) fn id<'a>(value: &'a Value, prefix: &str) -> Result<&'a str, Error> {
    value
        .as_str()
        .filter(|s| safe_id(s) && s.starts_with(prefix))
        .ok_or(Error::Rejected)
}
pub(crate) fn millis(value: &Value) -> Result<i64, Error> {
    let s = value
        .as_str()
        .filter(|s| s.len() <= 19)
        .ok_or(Error::Rejected)?;
    let ms = s
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0 && v.to_string() == s)
        .ok_or(Error::Rejected)?;
    timestamp(ms)?;
    Ok(ms)
}
fn timestamp(ms: i64) -> Result<String, Error> {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|_| Error::Rejected)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| Error::Rejected)
}
pub(crate) fn check_scope(
    row: &Value,
    cursor: &NativeLarkHistoryCursorPayload,
) -> Result<(), Error> {
    if id(&row["chat_id"], "oc_")? != cursor.origin.chat_id {
        return Err(Error::Rejected);
    }
    let thread = row
        .get("thread_id")
        .filter(|v| !v.is_null() && v.as_str() != Some(""));
    if let Some(thread) = thread {
        id(thread, "omt_")?;
    }
    if let Some(expected) = &cursor.origin.thread_id {
        if thread.and_then(Value::as_str) != Some(expected.as_str()) {
            return Err(Error::Rejected);
        }
    }
    Ok(())
}
pub(crate) fn project(
    value: &Value,
    cursor: &NativeLarkHistoryCursorPayload,
    mixed_media: bool,
) -> Result<Projection, Error> {
    let data = &value["data"];
    let rows = data["items"]
        .as_array()
        .filter(|v| v.len() <= usize::from(HISTORY_PAGE_SIZE))
        .ok_or(Error::Rejected)?;
    let more = data["has_more"].as_bool().ok_or(Error::Rejected)?;
    let next = if more {
        Some(
            data["page_token"]
                .as_str()
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= 1024
                        && !s.chars().any(char::is_control)
                        && *s != cursor.page_token
                })
                .ok_or(Error::Rejected)?
                .to_owned(),
        )
    } else {
        None
    };
    let mut result = Projection {
        messages: vec![],
        scanned: rows.len() as u8,
        omitted: 0,
        next,
        before: cursor.before_message_id.clone(),
        last_ms: cursor.last_scanned_ms,
        last_id: cursor.last_scanned_id.clone(),
        text_truncated: false,
    };
    let mut ids = BTreeSet::new();
    let mut budget = HISTORY_TEXT_BYTES;
    for row in rows {
        let message_id = id(&row["message_id"], "om_")?;
        let created = millis(&row["create_time"])?;
        if !ids.insert(message_id)
            || cursor.last_scanned_id.as_deref() == Some(message_id)
            || result.last_ms.is_some_and(|previous| created > previous)
        {
            return Err(Error::Rejected);
        }
        check_scope(row, cursor)?;
        result.last_ms = Some(created);
        result.last_id = Some(message_id.into());
        let deleted = row["deleted"].as_bool().ok_or(Error::Rejected)?;
        let updated = row["updated"].as_bool().ok_or(Error::Rejected)?;
        let kind = row["msg_type"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .ok_or(Error::Rejected)?;
        // Same-millisecond ordering is undocumented. No original or newer content escapes.
        if created >= cursor.anchor_created_ms || message_id == cursor.origin.message_id {
            result.omitted += 1;
            continue;
        }
        result.before = message_id.into();
        if deleted
            || kind == "system"
            || row
                .get("upper_message_id")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            result.omitted += 1;
            continue;
        }
        let sender = &row["sender"];
        let actor_kind = match sender["sender_type"].as_str() {
            Some("user") if sender["id_type"] == "open_id" => ActorKind::Human,
            Some("app") if sender["id_type"] == "app_id" => ActorKind::Bot,
            Some("anonymous" | "unknown") => {
                result.omitted += 1;
                continue;
            }
            _ => return Err(Error::Rejected),
        };
        if sender["tenant_key"].as_str() != Some(cursor.origin.identity.tenant_key.as_str()) {
            return Err(Error::Rejected);
        }
        let actor_id = id(
            &sender["id"],
            if actor_kind == ActorKind::Human {
                "ou_"
            } else {
                "cli_"
            },
        )?;
        if !matches!(kind, "text" | "post") {
            result.omitted += 1;
            continue;
        }
        let content: Value =
            serde_json::from_str(row["body"]["content"].as_str().ok_or(Error::Rejected)?)
                .map_err(|_| Error::Rejected)?;
        let mut omitted_content = vec![];
        let raw = if kind == "text" {
            content["text"].as_str().ok_or(Error::Rejected)?.to_owned()
        } else {
            let (text, resources) = super::content::parse(kind, &content)
                .map_err(|_| Error::Rejected)?
                .ok_or(Error::Rejected)?;
            // Keep text without inventing attachment facts or creating resource authority.
            // Old strict readers cannot decode Media; preserve legacy output while disabled.
            if !resources.is_empty() {
                if !mixed_media {
                    result.omitted += 1;
                    continue;
                }
                omitted_content.push(HistoryOmittedContent::Media);
            }
            text
        };
        let (text, truncated) =
            crate::discord::rest::history_projection::clip(&raw, budget.min(8192));
        budget -= text.len();
        result.text_truncated |= truncated;
        let edited_at = if updated {
            let update = millis(&row["update_time"])?;
            if update < created {
                return Err(Error::Rejected);
            }
            Some(timestamp(update)?)
        } else {
            None
        };
        result.messages.push(HistoryMessage {
            message_id: message_id.into(),
            author: Actor {
                id: actor_id.into(),
                kind: actor_kind,
                display_name: None,
            },
            text,
            occurred_at: timestamp(created)?,
            edited_at,
            attachments: vec![],
            omitted_content,
            text_truncated: truncated,
            metadata_truncated: false,
        });
    }
    result.messages.reverse();
    Ok(result)
}
