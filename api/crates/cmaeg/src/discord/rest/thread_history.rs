//! Native thread context is read-only and scoped by the caller's sealed event.
use super::{valid_id, DiscordRest, RestError};
use crate::tools::{ThreadHistory, ThreadHistoryMessage};
use cmaeg_core::{
    activity::{ActivityConversation, Actor, ActorKind},
    channel::Secret,
};
use serde_json::Value;
use std::collections::BTreeSet;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

impl DiscordRest {
    pub(crate) async fn thread_history(
        &self,
        token: &Secret,
        bot: &str,
        channel: &str,
        before: &str,
        conversation: &ActivityConversation,
    ) -> Result<ThreadHistory, RestError> {
        if !valid_id(channel)
            || !valid_id(before)
            || conversation.thread_id.as_deref() != Some(channel)
        {
            return Err(RestError::Rejected);
        }
        self.verify_bot(token, bot).await?;
        if self
            .verify_component_channel(token, channel, conversation)
            .await?
            .is_none()
        {
            return Err(RestError::Rejected);
        }
        // A deleted/inaccessible anchor cannot authorize a nearby arbitrary window.
        self.message(token, channel, before).await?;
        self.ready(token)?;
        let mut url = self.url(&format!("channels/{channel}/messages"))?;
        url.query_pairs_mut()
            .append_pair("before", before)
            .append_pair("limit", "20");
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        let body = self.decode(response, token, false).await?;
        project(&body, channel, before)
    }
}

fn project(body: &Value, channel: &str, before: &str) -> Result<ThreadHistory, RestError> {
    let refused = || RestError::Rejected;
    let boundary = before.parse::<u64>().map_err(|_| refused())?;
    let rows = body
        .as_array()
        .filter(|rows| rows.len() <= 20)
        .ok_or_else(refused)?;
    let mut ids = BTreeSet::new();
    let mut messages = Vec::new();
    let mut remaining = 32 * 1024;
    let mut text_truncated = false;
    for row in rows {
        let id = row["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(refused)?;
        let numeric = id.parse::<u64>().map_err(|_| refused())?;
        if numeric >= boundary
            || !ids.insert(numeric)
            || row["channel_id"].as_str() != Some(channel)
        {
            return Err(refused());
        }
        // System events are not human instructions. Keep ordinary user and bot
        // messages, including edits, without reusing MESSAGE_CREATE admission.
        if !matches!(row["type"].as_u64(), Some(0 | 19)) {
            continue;
        }
        let author = &row["author"];
        let author_id = author["id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(refused)?;
        let is_bot = match author.get("bot") {
            Some(Value::Bool(value)) => *value,
            None => false,
            _ => return Err(refused()),
        };
        let name = author["global_name"]
            .as_str()
            .or_else(|| author["username"].as_str())
            .map(|name| clip(name, 256).0);
        let raw = row["content"].as_str().ok_or_else(refused)?;
        let (text, clipped) = clip(raw, remaining.min(8192));
        remaining = remaining.saturating_sub(text.len());
        text_truncated |= clipped;
        let occurred_at = timestamp(&row["timestamp"])?;
        let edited_at = row
            .get("edited_timestamp")
            .filter(|value| !value.is_null())
            .map(timestamp)
            .transpose()?;
        let attachment_count = row["attachments"]
            .as_array()
            .filter(|items| items.len() <= 10)
            .ok_or_else(refused)?
            .len() as u8;
        messages.push((
            numeric,
            ThreadHistoryMessage {
                message_id: id.into(),
                author: Actor {
                    id: author_id.into(),
                    display_name: name,
                    kind: if is_bot {
                        ActorKind::Bot
                    } else if row.get("webhook_id").is_some_and(|value| !value.is_null()) {
                        ActorKind::Unknown
                    } else {
                        ActorKind::Human
                    },
                },
                text,
                occurred_at,
                edited_at,
                attachment_count,
            },
        ));
    }
    messages.sort_by_key(|(id, _)| *id);
    Ok(ThreadHistory {
        thread_id: channel.into(),
        before_message_id: before.into(),
        messages: messages.into_iter().map(|(_, message)| message).collect(),
        text_truncated,
    })
}
fn timestamp(value: &Value) -> Result<String, RestError> {
    let text = value
        .as_str()
        .filter(|text| text.len() <= 64)
        .ok_or(RestError::Rejected)?;
    OffsetDateTime::parse(text, &Rfc3339).map_err(|_| RestError::Rejected)?;
    Ok(text.into())
}
fn clip(text: &str, cap: usize) -> (String, bool) {
    let mut clean = String::new();
    for character in text
        .chars()
        .filter(|character| !cmaeg_core::activity::forbidden_text_char(*character))
    {
        if clean.len() + character.len_utf8() > cap {
            return (clean, true);
        }
        clean.push(character);
    }
    (clean, false)
}
