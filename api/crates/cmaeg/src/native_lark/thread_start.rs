//! Explicit replies may start only the exact admitted human root's group thread.
use super::{Client, Error};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ActivityKind, ConversationKind},
    channel::{native_lark::safe_id, NativeLarkReply, Secret},
    seal::{EventRefPayload, ReplyAddress},
};
use reqwest::Method;
use serde_json::{json, Value};

pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let ReplyAddress::NativeLark(reply) = &event.reply else {
        return false;
    };
    crate::interactions::native_lark::eligible(channel, event, activity)
        && channel.definition.routing.is_some()
        && activity.conversation.kind == ConversationKind::Group
        && reply.thread_id.is_none()
        && matches!(activity.kind, ActivityKind::Message { album_id: None, .. })
        && super::message::original_id(&reply.identity, &reply.message_id)
            .ok()
            .as_ref()
            == Some(&event.event_id)
}

fn optional_id<'a>(row: &'a Value, field: &str, prefix: &str) -> Result<Option<&'a str>, Error> {
    match row.get(field) {
        None => Ok(None),
        Some(Value::String(id)) if id.is_empty() => Ok(None),
        Some(Value::String(id)) if safe_id(id) && id.starts_with(prefix) => Ok(Some(id)),
        _ => Err(Error::Rejected),
    }
}

impl Client {
    /// The original message may already have acquired a thread since admission.
    /// Only that same human root can supply its current thread; never another parent.
    pub(crate) async fn thread_start_anchor(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        activity: &Activity,
    ) -> Result<Option<String>, Error> {
        let value = self
            .call(
                &reply.identity,
                Method::GET,
                &format!(
                    "/open-apis/im/v1/messages/{}?user_id_type=open_id",
                    reply.message_id
                ),
                Some(token),
                None,
            )
            .await?;
        let rows = value["data"]["items"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or(Error::Rejected)?;
        let row = &rows[0];
        let created = time::OffsetDateTime::parse(
            &activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| Error::Rejected)?
        .unix_timestamp_nanos()
            / 1_000_000;
        if row["message_id"].as_str() != Some(&reply.message_id)
            || row["chat_id"].as_str() != Some(&reply.chat_id)
            || row["deleted"] != false
            || row["create_time"].as_str() != Some(created.to_string().as_str())
            || row["sender"]["sender_type"] != "user"
            || row["sender"]["id_type"] != "open_id"
            || row["sender"]["id"].as_str() != Some(&activity.actor.id)
            || row["sender"]["tenant_key"].as_str() != Some(&reply.identity.tenant_key)
            || optional_id(row, "root_id", "om_")?.is_some_and(|id| id != reply.message_id)
            || optional_id(row, "parent_id", "om_")?.is_some()
        {
            return Err(Error::Rejected);
        }
        Ok(optional_id(row, "thread_id", "omt_")?.map(str::to_owned))
    }

    pub(crate) async fn start_thread_text(
        &self,
        token: &Secret,
        reply: &NativeLarkReply,
        known_thread: Option<&str>,
        text: &str,
    ) -> Result<NativeLarkReply, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        if reply.thread_id.is_some()
            || text.is_empty()
            || text.chars().count() > 4000
            || text.len() > 28 * 1024
        {
            return Err(Error::Invalid);
        }
        let value = self.call(&reply.identity, Method::POST,
            &format!("/open-apis/im/v1/messages/{}/reply", reply.message_id), Some(token),
            Some(json!({"msg_type":"text", "content":json!({"text":text}).to_string(), "reply_in_thread":true}))).await?;
        let row = &value["data"];
        let identity = || {
            let id = optional_id(row, "message_id", "om_")?.ok_or(Error::Unconfirmed)?;
            let thread = optional_id(row, "thread_id", "omt_")?.ok_or(Error::Unconfirmed)?;
            if id == reply.message_id
                || row["chat_id"].as_str() != Some(&reply.chat_id)
                || row["root_id"].as_str() != Some(&reply.message_id)
                || row["parent_id"].as_str() != Some(&reply.message_id)
                || known_thread.is_some_and(|expected| thread != expected)
                || row["msg_type"] != "text"
                || row["deleted"] != false
                || row["sender"]["sender_type"] != "app"
                || row["sender"]["id_type"] != "app_id"
                || row["sender"]["id"].as_str() != Some(&reply.identity.app_id)
                || row["sender"]["tenant_key"].as_str() != Some(&reply.identity.tenant_key)
            {
                return Err(Error::Unconfirmed);
            }
            let mut destination = reply.clone();
            destination.message_id = id.into();
            destination.thread_id = Some(thread.into());
            destination.validate().map_err(|_| Error::Unconfirmed)?;
            Ok(destination)
        };
        identity().map_err(|_| Error::Unconfirmed)
    }
}
