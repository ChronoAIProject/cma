//! Accept Telegram membership data only from a verified, exact relay group.
use super::{facts::Observation, payload::CallbackPayload, VerificationError};
use crate::ingress::{
    admission::Refusal,
    telegram::{member_joins, Outcome},
};
use cmaeg_core::channel::{Platform, Source};

pub(crate) fn observe(
    source: &Source,
    payload: &CallbackPayload,
) -> Option<Result<Observation, VerificationError>> {
    let Source::NyxidRelay {
        platform: Platform::Telegram,
        trust_normalized: false,
        bot_id: Some(bot_id),
        member_join_chat_ids,
        ..
    } = source
    else {
        return None;
    };
    let raw = payload.raw_platform_data.as_ref()?;
    if member_join_chat_ids.is_empty() || !member_joins::requested(raw) {
        return None;
    }
    Some(verified(payload, raw, bot_id, member_join_chat_ids))
}

fn verified(
    payload: &CallbackPayload,
    raw: &serde_json::Value,
    bot_id: &str,
    allowed_groups: &[String],
) -> Result<Observation, VerificationError> {
    let Outcome::Admitted(observed) = member_joins::observe(raw, bot_id, allowed_groups)? else {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    };
    let inviter = raw
        .pointer("/message/from/id")
        .and_then(serde_json::Value::as_i64);
    if payload.conversation.kind != "group"
        || payload.conversation.platform_id != observed.payload.conversation.platform_id
        || inviter.is_none_or(|id| id <= 0 || id.to_string() != payload.sender.platform_id)
        || payload.content.kind != "unknown"
        || payload.content.text.is_some()
        || payload.received_attachments() != 0
        || payload
            .thread_id
            .as_ref()
            .map(cmaeg_core::channel::Secret::as_str)
            != observed.facts.thread_id.as_deref()
    {
        return Err(VerificationError("telegram_member_join_relay_mismatch"));
    }
    let mut facts = observed.facts;
    if let Some(event) = &mut facts.event {
        event.source = format!("nyxid:route:{}", payload.conversation.id);
    }
    Ok(Observation::Message(facts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmaeg_core::activity::ActorKind;
    use serde_json::{json, Value};

    fn source() -> Source {
        serde_json::from_value(json!({
            "type": "nyxid_relay", "platform": "telegram", "bot_id": "77",
            "issuer": "https://nyx.example", "jwks_url": "https://nyx.example/.well-known/jwks.json",
            "key_id": "key-1", "route_ids": ["route-1"],
            "admission": {"type": "scoped", "senders": {"type":"open"},
                "chats": {"type":"allowlist", "ids":["-100","-101"]}, "groups":"all"},
            "member_join_chat_ids": ["-100"],
        }))
        .unwrap()
    }

    fn envelope() -> Value {
        json!({
            "message_id":"message-1", "correlation_id":"correlation-1", "platform":"telegram",
            "agent": {"api_key_id":"key-1","name":""},
            "conversation": {"id":"route-1","platform_id":"-100","type":"group"},
            "sender": {"platform_id":"7","display_name":"Inviter"},
            "content": {"type":"unknown"}, "timestamp":"2026-10-26T00:00:00Z",
            "raw_platform_data": {"update_id":43,"message":{
                "message_id":21,"date":1792972800,"chat":{"id":-100,"type":"supergroup"},
                "from":{"id":7,"is_bot":false},
                "new_chat_members":[{"id":10,"is_bot":false,"first_name":"Ada"},
                    {"id":11,"is_bot":false,"first_name":"Jo"},
                    {"id":77,"is_bot":true,"first_name":"Receiver"}],
            }},
        })
    }

    #[test]
    fn telegram_relay_member_joins_use_signed_group_and_inviter_correspondence() {
        let source = source();
        let body = envelope();
        let payload = serde_json::from_value(body.clone()).unwrap();
        let Observation::Message(facts) = observe(&source, &payload).unwrap().unwrap() else {
            panic!("expected joined members");
        };
        assert_eq!(facts.actor, ActorKind::System);
        let event = facts.event.unwrap();
        assert_eq!(event.id, "43");
        assert_eq!(event.source, "nyxid:route:route-1");
        assert_eq!(
            event.data,
            Some(json!({"members":[
                {"id":"10","display_name":"Ada","is_bot":false},
                {"id":"11","display_name":"Jo","is_bot":false},
            ]})),
        );

        for (field, value) in [
            ("/conversation/platform_id", json!("-101")),
            ("/conversation/type", json!("private")),
            ("/sender/platform_id", json!("8")),
            ("/content/type", json!("text")),
        ] {
            let mut wrong = body.clone();
            *wrong.pointer_mut(field).unwrap() = value;
            let payload = serde_json::from_value(wrong).unwrap();
            assert!(observe(&source, &payload).unwrap().is_err(), "{field}");
        }
    }
}
