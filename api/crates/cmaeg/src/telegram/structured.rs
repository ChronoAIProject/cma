//! Static structured replies use only the original retained bot/chat/topic.
use super::{media, media_token_matches, Error, TelegramClient};
use crate::{channels::resolver::ResolvedChannel, records::RecordClient};
use cmaeg_core::{
    activity::SourceKind,
    channel::Secret,
    message::StructuredContent,
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn authorized(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> bool {
    let ReplyAddress::Telegram { bot_id, .. } = &event.reply else {
        return false;
    };
    if !channel
        .definition
        .secrets
        .telegram_bot_token
        .as_ref()
        .is_some_and(|token| media_token_matches(token, bot_id))
    {
        return false;
    }
    let Ok(plan) =
        crate::turns::routed::retained_received_source(records, keys, channel, event).await
    else {
        return false;
    };
    matches!(&plan.activity.source, SourceKind::Telegram { bot_id: original, .. } if original == bot_id)
}

impl TelegramClient {
    /// One platform attempt, with no fetch, geocoding, live tracking or retry.
    pub(crate) async fn send_structured(
        &self,
        token: &Secret,
        bot: &str,
        chat: &str,
        thread: Option<&str>,
        content: &StructuredContent,
    ) -> Result<String, Error> {
        if !media_token_matches(token, bot)
            || content.validate().is_err()
            || !chat
                .parse::<i64>()
                .is_ok_and(|n| n != 0 && n.to_string() == chat)
            || thread.is_some_and(|s| !s.parse::<i64>().is_ok_and(|n| n > 0 && n.to_string() == s))
        {
            return Err(Error::Rejected);
        }
        let (method, expected) = match content {
            StructuredContent::Contact { .. } => ("sendContact", "contact"),
            StructuredContent::Location { .. } => ("sendLocation", "location"),
            StructuredContent::Venue { .. } => ("sendVenue", "venue"),
        };
        let mut body = serde_json::to_value(content).map_err(|_| Error::Rejected)?;
        body.as_object_mut().ok_or(Error::Rejected)?.remove("type");
        body["chat_id"] = chat.into();
        if let Some(thread) = thread {
            body["message_thread_id"] = thread.into();
        }
        let value = self.call(token, method, &body).await?;
        let id = media::message_identity(&value, chat, thread)?;
        if !value[expected].is_object() || !sender_matches(&value, bot, chat) {
            return Err(Error::Unconfirmed);
        }
        if expected == "location" && value[expected].get("live_period").is_some() {
            return Err(Error::Unconfirmed);
        }
        Ok(id)
    }
}
fn sender_matches(value: &Value, bot: &str, chat: &str) -> bool {
    if value.get("from").is_some() {
        return value["from"]["is_bot"] == true
            && value["from"]["id"]
                .as_i64()
                .is_some_and(|id| id.to_string() == bot);
    }
    value["chat"]["type"] == "channel"
        && value.get("sender_chat").is_none_or(|sender| {
            sender["type"] == "channel"
                && sender["id"]
                    .as_i64()
                    .is_some_and(|id| id.to_string() == chat)
        })
}
