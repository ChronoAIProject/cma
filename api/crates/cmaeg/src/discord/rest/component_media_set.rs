//! Exact ordered attachment membership; a confirmed clear grants no media authority.
use super::{component_edit::matches_components, media_card_snapshot, RestError};
use crate::native_media::model::{Presentation, Snapshot};
use cmaeg_core::message::{NativeMediaKind, MAX_NATIVE_ATTACHMENTS};
use serde_json::Value;
use std::collections::BTreeSet;

#[allow(clippy::too_many_arguments)]
pub(crate) fn media_card_snapshots(
    value: &Value,
    bot: &str,
    channel: &str,
    message: &str,
    kinds: &[NativeMediaKind],
    text: &str,
    components: &Value,
) -> Result<Vec<Snapshot>, RestError> {
    let files = value["attachments"].as_array().ok_or(RestError::Unknown)?;
    if kinds.is_empty() || kinds.len() > MAX_NATIVE_ATTACHMENTS || files.len() != kinds.len() {
        return Err(RestError::Unknown);
    }
    let mut ids = BTreeSet::new();
    let mut total = 0_u64;
    let mut result = Vec::with_capacity(files.len());
    for (file, kind) in files.iter().zip(kinds) {
        if !ids.insert(file["id"].as_str().ok_or(RestError::Unknown)?) {
            return Err(RestError::Unknown);
        }
        // Each existing scalar proof also binds the complete message presentation.
        // Project one attachment without weakening content/components or bot custody.
        let mut projected = value.clone();
        projected["attachments"] = Value::Array(vec![file.clone()]);
        let snapshot =
            media_card_snapshot(&projected, bot, channel, message, *kind, text, components)?;
        total = total
            .checked_add(snapshot.files[0].size.ok_or(RestError::Unknown)?)
            .ok_or(RestError::Unknown)?;
        if total > crate::media::outbound::MAX_OUTBOUND_BYTES {
            return Err(RestError::Unknown);
        }
        result.push(snapshot);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn clear_card_confirmed(
    value: &Value,
    bot: &str,
    channel: &str,
    message: &str,
    text: &str,
    components: &Value,
    before: &Snapshot,
) -> Result<(), RestError> {
    if value["attachments"]
        .as_array()
        .is_none_or(|files| !files.is_empty())
        || value["id"].as_str() != Some(message)
        || value["channel_id"].as_str() != Some(channel)
        || value["author"]["id"].as_str() != Some(bot)
        || value["author"]["bot"] != true
        || value["content"].as_str() != Some(text)
        || !matches_components(components, &value["components"])
        || !matches!(value["type"].as_u64(), Some(0 | 19))
    {
        return Err(RestError::Unknown);
    }
    for key in [
        "webhook_id",
        "interaction",
        "interaction_metadata",
        "message_snapshots",
        "poll",
        "call",
        "activity",
        "application",
    ] {
        if value.get(key).is_some_and(|value| !value.is_null()) {
            return Err(RestError::Unknown);
        }
    }
    if value
        .get("sticker_items")
        .is_some_and(|value| value.as_array().is_none_or(|items| !items.is_empty()))
    {
        return Err(RestError::Unknown);
    }
    let (after, eligible) = super::media_presentation::observe(value, &serde_json::json!({}))?;
    if !eligible || !same_message_presentation(before.presentation.as_ref(), Some(&after)) {
        return Err(RestError::Unknown);
    }
    Ok(())
}

pub(super) fn same_message_presentation(
    before: Option<&Presentation>,
    after: Option<&Presentation>,
) -> bool {
    match (before, after) {
        (
            Some(Presentation::Discord {
                message_type: old_type,
                reply: old_reply,
                flags: old_flags,
                ..
            }),
            Some(Presentation::Discord {
                message_type,
                reply,
                flags,
                ..
            }),
        ) => message_type == old_type && reply == old_reply && flags == old_flags,
        _ => false,
    }
}
