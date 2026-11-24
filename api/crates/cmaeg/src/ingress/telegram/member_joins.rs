//! Normalize one authenticated group join update into a single system event.
use super::{Observed, Outcome};
use crate::ingress::{
    admission::Refusal,
    relay::{
        facts::RelayFacts,
        payload::{CallbackPayload, Sender},
        VerificationError,
    },
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, ConversationKind, SourceKind},
    channel::{Platform, Source},
    cloudevent::CloudEvent,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const MAX_MEMBERS: usize = 50;
pub(crate) const EVENT_TYPE: &str = "telegram.group_members_joined";

pub(crate) fn is_member_join(activity: &Activity) -> bool {
    matches!(
        activity.source,
        SourceKind::Telegram { .. }
            | SourceKind::NyxidRelay {
                platform: Platform::Telegram,
                ..
            }
    ) && matches!(&activity.kind, ActivityKind::Event { ce, .. } if ce.r#type == EVENT_TYPE)
}

pub(crate) fn group_opted_in(source: &Source, group_id: &str) -> bool {
    matches!(source,
        Source::Telegram { message_events: true, member_join_chat_ids, .. }
        | Source::NyxidRelay { platform: Platform::Telegram, member_join_chat_ids, .. }
        if member_join_chat_ids.iter().any(|id| id == group_id))
}

pub(crate) fn relay_messaging_source(source: &Source) -> Source {
    let mut source = source.clone();
    if let Source::NyxidRelay {
        member_join_chat_ids,
        ..
    } = &mut source
    {
        member_join_chat_ids.clear();
    }
    source
}

pub(crate) fn requested(raw: &Value) -> bool {
    raw.get("message")
        .is_some_and(|message| message.get("new_chat_members").is_some())
}

pub(crate) fn observe(
    raw: &Value,
    bot_id: &str,
    opted_in_chats: &[String],
) -> Result<Outcome, VerificationError> {
    let invalid = || VerificationError("telegram_member_join_invalid");
    let system_event = || Ok(Outcome::Refused(Refusal::SystemEvent));
    let message = &raw["message"];
    let chat = &message["chat"];
    let Some(chat_id) = chat["id"].as_i64().filter(|id| *id < 0) else {
        return system_event();
    };
    let chat_id = chat_id.to_string();
    if !matches!(chat["type"].as_str(), Some("group" | "supergroup"))
        || !opted_in_chats.contains(&chat_id)
        || raw.as_object().is_none_or(|object| object.len() != 2)
        || message.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "message_id"
                        | "message_thread_id"
                        | "is_topic_message"
                        | "date"
                        | "chat"
                        | "from"
                        | "sender_chat"
                        | "new_chat_members"
                )
            })
        })
    {
        return system_event();
    }
    let update_id = raw["update_id"]
        .as_i64()
        .filter(|id| *id >= 0)
        .ok_or_else(invalid)?;
    let message_id = positive_id(&message["message_id"]).ok_or_else(invalid)?;
    let thread_id = match (
        message.get("is_topic_message"),
        message.get("message_thread_id"),
    ) {
        (Some(Value::Bool(true)), Some(id)) if chat["type"] == "supergroup" => {
            Some(positive_id(id).ok_or_else(invalid)?.to_string())
        }
        (None | Some(Value::Bool(false)), None) => None,
        _ => return system_event(),
    };
    let date = positive_id(&message["date"]).ok_or_else(invalid)?;
    let timestamp = OffsetDateTime::from_unix_timestamp(date)
        .map_err(|_| invalid())?
        .format(&Rfc3339)
        .map_err(|_| invalid())?;
    let Some(members) = message["new_chat_members"]
        .as_array()
        .filter(|members| (1..=MAX_MEMBERS).contains(&members.len()))
    else {
        return system_event();
    };
    let mut seen = BTreeSet::new();
    let mut humans = Vec::new();
    for member in members {
        let id = positive_id(&member["id"]).ok_or_else(invalid)?.to_string();
        let is_bot = member["is_bot"].as_bool().ok_or_else(invalid)?;
        if is_bot || id == bot_id || !seen.insert(id.clone()) {
            continue;
        }
        let first = member["first_name"].as_str().ok_or_else(invalid)?;
        let mut name = first.to_owned();
        if let Some(last) = member.get("last_name").and_then(Value::as_str) {
            name.push(' ');
            name.push_str(last);
        }
        let mut sender = Sender {
            platform_id: id,
            display_name: Some(name),
        };
        sender.repair_display_name();
        humans.push(json!({
            "id": sender.platform_id,
            "display_name": sender.display_name,
            "is_bot": false,
        }));
    }
    if humans.is_empty() {
        return system_event();
    }
    let payload: CallbackPayload = serde_json::from_value(json!({
        "message_id": format!("telegram:{bot_id}:{update_id}"),
        "correlation_id": update_id.to_string(),
        "platform": "telegram",
        "agent": {"api_key_id": bot_id, "name": ""},
        "conversation": {
            "id": crate::telegram::route(bot_id, &chat_id),
            "platform_id": chat_id,
            "type": chat["type"],
        },
        "sender": {"platform_id": "telegram:system"},
        "content": {"type": "unknown"},
        "timestamp": timestamp,
    }))
    .map_err(|_| invalid())?;
    payload.validate()?;
    let ce = CloudEvent {
        specversion: "1.0".into(),
        id: update_id.to_string(),
        source: format!("telegram:bot:{bot_id}"),
        r#type: EVENT_TYPE.into(),
        time: Some(timestamp),
        subject: Some(format!("telegram:chat:{chat_id}")),
        datacontenttype: Some("application/json".into()),
        dataschema: None,
        data: Some(json!({"members": humans})),
        extensions: Default::default(),
    };
    Ok(Outcome::Admitted(Box::new(Observed {
        payload,
        update_id,
        message_id: message_id.to_string(),
        facts: RelayFacts {
            thread: None,
            event: Some(Box::new(ce)),
            callback: None,
            conversation: ConversationKind::Group,
            thread_id,
            mentioned: Some(false),
            replied_to_bot: Some(false),
            actor: ActorKind::System,
            text: None,
            attachments: Vec::new(),
            album_id: None,
        },
    })))
}

fn positive_id(value: &Value) -> Option<i64> {
    value.as_i64().filter(|id| *id > 0)
}
