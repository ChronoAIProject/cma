//! Native payload context is bounded data. It never drives routing or authority.
use cmaeg_core::activity::{
    Activity, ActivityKind, QuoteRelation, QuoteTextOrigin, QuotedContent,
    QuotedContentKind as Kind, QuotedMessage, QuotedText, SourceKind, MAX_QUOTED_TEXT_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default)]
pub(crate) enum Intake {
    #[default]
    Legacy,
    Candidate(Option<QuotedMessage>),
    /// Conflicting optional facts are ignored only by a retained legacy projection.
    Ambiguous,
    /// Discord fixes the projection before acknowledging its Gateway sequence.
    Pinned(Option<Snapshot>),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_message: Option<QuotedMessage>,
}
impl Snapshot {
    pub fn apply(&self, activity: &mut Activity) -> Result<(), ()> {
        if self.version != 1 {
            return Err(());
        }
        let ActivityKind::Message { quoted_message, .. } = &mut activity.kind else {
            return Err(());
        };
        *quoted_message = self.quoted_message.clone();
        activity.validate().map_err(|_| ())
    }
}
fn text(value: &Value, origin: QuoteTextOrigin) -> Option<QuotedText> {
    let s = value.as_str()?;
    let mut end = s.len().min(MAX_QUOTED_TEXT_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(QuotedText {
        text: s[..end].into(),
        truncated: end < s.len(),
        origin,
    })
}
fn quote(id: String, content: Option<QuotedContent>, source: &SourceKind) -> Option<QuotedMessage> {
    let result = QuotedMessage {
        relation: if matches!(source, SourceKind::Whatsapp { .. }) {
            QuoteRelation::Context
        } else {
            QuoteRelation::Reply
        },
        message_id: id,
        content,
    };
    result.validate(source).ok().map(|()| result)
}
/// Only the native same-chat parent. external_reply, forward_origin, and aliases
/// never become an ordinary reply. A selected quote is sender-chosen content.
pub(crate) fn telegram(raw: &Value, activity: &Activity) -> Option<QuotedMessage> {
    if !matches!(activity.source, SourceKind::Telegram { .. }) {
        return None;
    }
    let message = raw.get("message").or_else(|| raw.get("channel_post"))?;
    let parent = message.get("reply_to_message")?;
    let chat = message["chat"]["id"].as_i64()?;
    if chat.to_string() != activity.conversation.id
        || parent["chat"]["id"].as_i64() != Some(chat)
        || message.get("message_thread_id") != parent.get("message_thread_id")
    {
        return None;
    }
    let id = parent["message_id"].as_i64()?.to_string();
    if parent["message_id"] == message["message_id"] {
        return None;
    }
    let excerpt = text(&message["quote"]["text"], QuoteTextOrigin::SelectedQuote)
        .or_else(|| text(&parent["text"], QuoteTextOrigin::ParentText))
        .or_else(|| text(&parent["caption"], QuoteTextOrigin::ParentCaption));
    let mut kinds = Vec::new();
    for (field, kind) in [
        ("photo", Kind::Image),
        ("video", Kind::Video),
        ("animation", Kind::Video),
        ("video_note", Kind::Video),
        ("audio", Kind::Audio),
        ("voice", Kind::Audio),
        ("document", Kind::File),
        ("sticker", Kind::Sticker),
        ("contact", Kind::Contact),
        ("location", Kind::Location),
        ("venue", Kind::Location),
        ("poll", Kind::Poll),
    ] {
        if parent.get(field).is_some_and(|v| !v.is_null()) && !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    quote(
        id,
        Some(QuotedContent {
            text: excerpt,
            kinds,
        }),
        &activity.source,
    )
}
/// A real REPLY/DEFAULT in this channel/guild; never a forward or crosspost.
pub(crate) fn discord(raw: &Value, activity: &Activity) -> Option<QuotedMessage> {
    if !matches!(activity.source, SourceKind::Discord { .. }) || raw["type"].as_u64() != Some(19) {
        return None;
    }
    let reference = raw.get("message_reference")?;
    if reference.get("type").is_some_and(|v| v.as_u64() != Some(0))
        || reference["channel_id"].as_str() != Some(&activity.conversation.id)
        || raw["channel_id"].as_str() != Some(&activity.conversation.id)
        || reference.get("guild_id") != raw.get("guild_id")
    {
        return None;
    }
    let id = reference["message_id"].as_str()?.to_owned();
    if raw["id"].as_str() == Some(&id) {
        return None;
    }
    let content = match raw.get("referenced_message").filter(|v| !v.is_null()) {
        None => None,
        Some(parent) => {
            if parent["id"].as_str() != Some(&id)
                || parent["channel_id"].as_str() != Some(&activity.conversation.id)
                || parent
                    .get("guild_id")
                    .is_some_and(|g| Some(g) != raw.get("guild_id"))
            {
                return None;
            }
            let mut kinds = Vec::new();
            if let Some(files) = parent["attachments"].as_array() {
                for file in files.iter().take(10) {
                    let kind = match file["content_type"]
                        .as_str()
                        .unwrap_or("")
                        .split('/')
                        .next()
                    {
                        Some("image") => Kind::Image,
                        Some("video") => Kind::Video,
                        Some("audio") => Kind::Audio,
                        _ => Kind::File,
                    };
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
            }
            if parent["sticker_items"]
                .as_array()
                .is_some_and(|s| !s.is_empty())
            {
                kinds.push(Kind::Sticker);
            }
            if parent.get("poll").is_some_and(|v| !v.is_null()) {
                kinds.push(Kind::Poll);
            }
            Some(QuotedContent {
                text: text(&parent["content"], QuoteTextOrigin::ParentText),
                kinds,
            })
        }
    };
    quote(id, content, &activity.source)
}
pub(crate) fn lark(raw: &Value) -> Option<QuotedMessage> {
    let id = raw["event"]["message"]["parent_id"].as_str()?;
    (id.starts_with("om_")
        && cmaeg_core::channel::native_lark::safe_id(id)
        && raw["event"]["message"]["message_id"].as_str() != Some(id))
    .then(|| QuotedMessage {
        relation: QuoteRelation::Reply,
        message_id: id.into(),
        content: None,
    })
}
pub(crate) fn whatsapp(
    body: &[u8],
    activity: &Activity,
    current_id: &str,
) -> Option<QuotedMessage> {
    let SourceKind::Whatsapp {
        waba_id,
        phone_number_id,
        ..
    } = &activity.source
    else {
        return None;
    };
    // Groups have separate relationship semantics; this version handles private intake.
    if activity.conversation.id != activity.actor.id {
        return None;
    }
    let raw: Value = serde_json::from_slice(body).ok()?;
    let mut found = None;
    for entry in raw["entry"].as_array()? {
        if entry["id"].as_str() != Some(waba_id) {
            continue;
        }
        for change in entry["changes"].as_array()? {
            let value = &change["value"];
            if change["field"] != "messages"
                || value["metadata"]["phone_number_id"].as_str() != Some(phone_number_id)
            {
                continue;
            }
            for message in value["messages"].as_array().into_iter().flatten() {
                if message["id"].as_str() != Some(current_id)
                    || message["from"].as_str() != Some(&activity.actor.id)
                {
                    continue;
                }
                let context = &message["context"];
                if context.get("referred_product").is_some()
                    || context.get("forwarded").is_some()
                    || context.get("frequently_forwarded").is_some()
                {
                    continue;
                }
                let Some(id) = context["id"].as_str().filter(|id| *id != current_id) else {
                    continue;
                };
                // The provider's context.from is the quoted sender, never the current actor.
                if !context["from"]
                    .as_str()
                    .is_some_and(cmaeg_core::channel::whatsapp::valid_meta_id)
                {
                    continue;
                }
                let candidate = quote(id.into(), None, &activity.source)?;
                if found.as_ref().is_some_and(|old| old != &candidate) {
                    return None;
                }
                found = Some(candidate);
            }
        }
    }
    found
}

/// Find ambiguous context before any per-message mutation in a signed Meta batch.
pub(crate) fn whatsapp_batch_conflicts(
    body: &[u8],
    source: &cmaeg_core::channel::Source,
) -> Vec<String> {
    let Some(source_kind) = super::whatsapp::source_kind(source) else {
        return Vec::new();
    };
    let SourceKind::Whatsapp {
        waba_id,
        phone_number_id,
        ..
    } = &source_kind
    else {
        return Vec::new();
    };
    let Ok(raw) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeMap::new();
    let mut conflicts = std::collections::BTreeSet::new();
    for entry in raw["entry"].as_array().into_iter().flatten() {
        if entry["id"].as_str() != Some(waba_id) {
            continue;
        }
        for change in entry["changes"].as_array().into_iter().flatten() {
            let value = &change["value"];
            if change["field"] != "messages"
                || value["messaging_product"] != "whatsapp"
                || value["metadata"]["phone_number_id"].as_str() != Some(phone_number_id)
            {
                continue;
            }
            for message in change["value"]["messages"].as_array().into_iter().flatten() {
                if message.get("group_id").is_some()
                    || !matches!(
                        message["type"].as_str(),
                        Some(
                            "text"
                                | "image"
                                | "video"
                                | "audio"
                                | "document"
                                | "sticker"
                                | "contacts"
                                | "location"
                        )
                    )
                {
                    continue;
                }
                let Some(id) = message["id"].as_str() else {
                    continue;
                };
                let context = &message["context"];
                let quote = context["id"]
                    .as_str()
                    .filter(|parent| *parent != id)
                    .filter(|_| {
                        context["from"]
                            .as_str()
                            .is_some_and(cmaeg_core::channel::whatsapp::valid_meta_id)
                            && context.get("referred_product").is_none()
                            && context.get("forwarded").is_none()
                            && context.get("frequently_forwarded").is_none()
                    })
                    .and_then(|id| quote(id.into(), None, &source_kind));
                if seen
                    .insert(id, quote.clone())
                    .is_some_and(|old| old != quote)
                {
                    conflicts.insert(id.to_owned());
                }
            }
        }
    }
    conflicts.into_iter().collect()
}

#[cfg(test)]
#[path = "quoted_context_tests.rs"]
mod tests;
