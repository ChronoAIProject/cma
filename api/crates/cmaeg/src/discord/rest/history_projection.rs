//! Strict page validation and bounded, authority-free message facts.
use super::{valid_id, RestError};
use cmaeg_core::{
    activity::{Actor, ActorKind},
    conversation_history::{
        HistoryAttachmentFacts, HistoryMessage, HistoryOmittedContent, HISTORY_METADATA_BYTES,
        HISTORY_PAGE_SIZE, HISTORY_TEXT_BYTES,
    },
};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) struct Projection {
    pub messages: Vec<HistoryMessage>,
    pub scanned: u8,
    pub omitted: u8,
    pub next: Option<String>,
    pub text_truncated: bool,
    pub metadata_truncated: bool,
}

pub(crate) fn project(body: &Value, channel: &str, before: &str) -> Result<Projection, RestError> {
    let mut previous = snowflake(before)?;
    let rows = body
        .as_array()
        .filter(|rows| rows.len() <= 26)
        .ok_or(RestError::Rejected)?;
    let mut text_budget = HISTORY_TEXT_BYTES;
    let mut metadata_budget = HISTORY_METADATA_BYTES;
    let mut result = Projection {
        messages: vec![],
        scanned: rows.len().min(usize::from(HISTORY_PAGE_SIZE)) as u8,
        omitted: 0,
        next: None,
        text_truncated: false,
        metadata_truncated: false,
    };
    for (index, row) in rows.iter().enumerate() {
        let id = row["id"].as_str().ok_or(RestError::Rejected)?;
        let numeric = snowflake(id)?;
        if numeric >= previous || row["channel_id"].as_str() != Some(channel) {
            return Err(RestError::Rejected);
        }
        previous = numeric;
        let kind = row["type"]
            .as_u64()
            .filter(|v| *v <= u64::from(u8::MAX))
            .ok_or(RestError::Rejected)?;
        // Validate the lookahead as well, but never project it or consume its budget.
        let mut extra_text = HISTORY_TEXT_BYTES;
        let mut extra_metadata = HISTORY_METADATA_BYTES;
        let ordinary = matches!(kind, 0 | 19);
        let projected = message(
            row,
            if index < 25 && ordinary {
                &mut text_budget
            } else {
                &mut extra_text
            },
            if index < 25 && ordinary {
                &mut metadata_budget
            } else {
                &mut extra_metadata
            },
        )?;
        if index >= usize::from(HISTORY_PAGE_SIZE) {
            continue;
        }
        if rows.len() > usize::from(HISTORY_PAGE_SIZE) {
            result.next = Some(id.into());
        }
        if ordinary {
            result.text_truncated |= projected.text_truncated;
            result.metadata_truncated |= projected.metadata_truncated;
            result.messages.push(projected);
        } else {
            result.omitted += 1;
        }
    }
    result.messages.reverse();
    Ok(result)
}

fn message(
    row: &Value,
    text_budget: &mut usize,
    metadata_budget: &mut usize,
) -> Result<HistoryMessage, RestError> {
    let author = &row["author"];
    let author_id = author["id"].as_str().ok_or(RestError::Rejected)?;
    snowflake(author_id)?;
    let bot = optional_bool(author, "bot")?.unwrap_or(false);
    let system = optional_bool(author, "system")?.unwrap_or(false);
    let webhook = optional_string(row, "webhook_id")?;
    if let Some(id) = webhook {
        snowflake(id)?;
    }
    let mut metadata_truncated = false;
    let name = optional_string(author, "global_name")?.or(optional_string(author, "username")?);
    let display_name =
        name.map(|name| metadata(name, 256, metadata_budget, &mut metadata_truncated));
    let raw = row["content"].as_str().ok_or(RestError::Rejected)?;
    let (text, text_truncated) = clip(raw, (*text_budget).min(8192));
    *text_budget -= text.len();
    let occurred_at = timestamp(&row["timestamp"])?;
    let edited_at = row
        .get("edited_timestamp")
        .filter(|v| !v.is_null())
        .map(timestamp)
        .transpose()?;
    let raw_attachments = row["attachments"]
        .as_array()
        .filter(|v| v.len() <= 10)
        .ok_or(RestError::Rejected)?;
    let mut ids = std::collections::BTreeSet::new();
    let mut attachments = Vec::new();
    for value in raw_attachments {
        let attachment = attachment(value, metadata_budget, &mut metadata_truncated)?;
        if !ids.insert(attachment.attachment_id.clone()) {
            return Err(RestError::Rejected);
        }
        attachments.push(attachment);
    }
    let mut omitted_content = vec![];
    for (key, flag) in [
        ("embeds", HistoryOmittedContent::Embeds),
        ("components", HistoryOmittedContent::Components),
        ("sticker_items", HistoryOmittedContent::Stickers),
        ("stickers", HistoryOmittedContent::Stickers),
        (
            "message_snapshots",
            HistoryOmittedContent::ForwardedSnapshot,
        ),
    ] {
        if let Some(value) = row.get(key) {
            let values = value.as_array().ok_or(RestError::Rejected)?;
            if !values.is_empty() && !omitted_content.contains(&flag) {
                omitted_content.push(flag);
            }
        }
    }
    if let Some(poll) = row.get("poll").filter(|v| !v.is_null()) {
        if !poll.is_object() {
            return Err(RestError::Rejected);
        }
        omitted_content.push(HistoryOmittedContent::Poll);
    }
    Ok(HistoryMessage {
        message_id: row["id"].as_str().ok_or(RestError::Rejected)?.into(),
        author: Actor {
            id: author_id.into(),
            display_name,
            kind: if system || webhook.is_some() {
                ActorKind::Unknown
            } else if bot {
                ActorKind::Bot
            } else {
                ActorKind::Human
            },
        },
        text,
        occurred_at,
        edited_at,
        attachments,
        omitted_content,
        text_truncated,
        metadata_truncated,
    })
}

fn attachment(
    value: &Value,
    budget: &mut usize,
    truncated: &mut bool,
) -> Result<HistoryAttachmentFacts, RestError> {
    let id = value["id"].as_str().ok_or(RestError::Rejected)?;
    snowflake(id)?;
    let filename = value["filename"].as_str().ok_or(RestError::Rejected)?;
    let filename = metadata(filename, 1024, budget, truncated);
    let mime_type =
        optional_string(value, "content_type")?.map(|v| metadata(v, 256, budget, truncated));
    let description =
        optional_string(value, "description")?.map(|v| metadata(v, 4096, budget, truncated));
    let dimension = |key| {
        value
            .get(key)
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or(RestError::Rejected)
            })
            .transpose()
    };
    let duration_seconds = value
        .get("duration_secs")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or(RestError::Rejected)
        })
        .transpose()?;
    Ok(HistoryAttachmentFacts {
        attachment_id: id.into(),
        filename,
        mime_type,
        description,
        size_bytes: value["size"].as_u64().ok_or(RestError::Rejected)?,
        width: dimension("width")?,
        height: dimension("height")?,
        duration_seconds,
        ephemeral: optional_bool(value, "ephemeral")?,
    })
}
fn snowflake(value: &str) -> Result<u64, RestError> {
    if !valid_id(value) {
        return Err(RestError::Rejected);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or(RestError::Rejected)
}
fn optional_bool(value: &Value, key: &str) -> Result<Option<bool>, RestError> {
    value
        .get(key)
        .map(|v| v.as_bool().ok_or(RestError::Rejected))
        .transpose()
}
fn optional_string<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, RestError> {
    value
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().ok_or(RestError::Rejected))
        .transpose()
}
fn timestamp(value: &Value) -> Result<String, RestError> {
    let text = value
        .as_str()
        .filter(|v| v.len() <= 64)
        .ok_or(RestError::Rejected)?;
    OffsetDateTime::parse(text, &Rfc3339).map_err(|_| RestError::Rejected)?;
    Ok(text.into())
}
fn metadata(value: &str, cap: usize, budget: &mut usize, truncated: &mut bool) -> String {
    let (text, clipped) = clip(value, cap.min(*budget));
    *budget -= text.len();
    *truncated |= clipped;
    text
}
fn clip(value: &str, cap: usize) -> (String, bool) {
    let mut text = String::new();
    let mut changed = false;
    for ch in value.chars() {
        if cmaeg_core::activity::forbidden_text_char(ch) {
            changed = true;
            continue;
        }
        if text.len() + ch.len_utf8() > cap {
            return (text, true);
        }
        text.push(ch);
    }
    (text, changed)
}

#[cfg(test)]
mod tests;
