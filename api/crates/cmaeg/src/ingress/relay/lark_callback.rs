//! Signed Lark/Feishu card actions are candidates, never authority by themselves.
//! Only a current CMA-issued card can turn one into an interaction.
use super::{
    facts::{CallbackFacts, Observation, RelayFacts},
    payload::CallbackPayload,
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, ConversationKind};
use serde_json::{json, Value};

const INVALID: VerificationError = VerificationError("lark_callback_invalid");

pub(super) fn observe(
    payload: &CallbackPayload,
    raw: &Value,
) -> Result<Observation, VerificationError> {
    let event = raw.get("event").ok_or(INVALID)?;
    let context = event.get("context").ok_or(INVALID)?;
    let actor = id(event.pointer("/operator/open_id"))?;
    let chat = id(context.get("open_chat_id"))?;
    let message = id(context.get("open_message_id"))?;
    let callback = id(raw.pointer("/header/event_id"))?;
    if !actor.starts_with("ou_")
        || actor != payload.sender.platform_id
        || chat != payload.conversation.platform_id
        || payload.reply_to_platform_message_id.as_deref() != Some(message)
        || payload.content.kind != "card_action"
        || payload.received_attachments() != 0
        || event.get("host").is_some_and(|host| host != "im_message")
    {
        return Err(INVALID);
    }
    let action = event.get("action").ok_or(INVALID)?;
    let tag = action.get("tag").and_then(Value::as_str);
    if !matches!(tag, Some("button" | "form_submit")) {
        return Ok(Observation::Refused(Refusal::InteractiveActionUnsupported));
    }
    let Some(value) = action.get("value") else {
        return Ok(Observation::Refused(Refusal::InteractiveActionUnsupported));
    };
    let Some(data) = value
        .get("cma_action")
        .and_then(Value::as_str)
        .filter(|s| (s.starts_with("cl1:") || s.starts_with("clf1:")) && s.len() <= 64)
    else {
        return Ok(Observation::Refused(Refusal::InteractiveActionUnsupported));
    };
    if value.as_object().is_none_or(|v| v.len() != 1) {
        return Err(INVALID);
    }
    let submitted = action.get("form_value").filter(|value| !value.is_null());
    let form = if data.starts_with("clf1:") {
        let Some(fields) = submitted
            .and_then(Value::as_object)
            .filter(|fields| fields.len() <= 12)
        else {
            return Ok(Observation::Refused(Refusal::InteractiveActionUnsupported));
        };
        if serde_json::to_vec(fields).map_err(|_| INVALID)?.len()
            > crate::interactions::lark_form::MAX_SUBMITTED_BYTES
        {
            return Err(INVALID);
        }
        Some(fields.clone())
    } else {
        if submitted.is_some() || tag != Some("button") {
            return Ok(Observation::Refused(Refusal::InteractiveActionUnsupported));
        }
        None
    };
    let normalized =
        json!({"tag":tag,"value":value,"form_value":submitted,"open_message_id":message});
    if payload
        .content
        .text
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        != Some(normalized)
    {
        return Err(INVALID);
    }
    let conversation = match context.get("chat_type").and_then(Value::as_str) {
        Some("p2p") => ConversationKind::Private,
        Some("group") | None => ConversationKind::Group,
        _ => return Err(INVALID),
    };
    Ok(Observation::Message(RelayFacts {
        thread: None,
        event: None,
        callback: Some(Box::new(CallbackFacts {
            id: callback.into(),
            data: data.into(),
            message_id: message.into(),
            form,
        })),
        conversation,
        thread_id: None,
        mentioned: Some(true),
        replied_to_bot: Some(true),
        actor: ActorKind::Human,
        text: Some(String::new()),
        attachments: vec![],
        album_id: None,
    }))
}

fn id(value: Option<&Value>) -> Result<&str, VerificationError> {
    value
        .and_then(Value::as_str)
        .filter(|s| super::payload::identifier(s))
        .ok_or(INVALID)
}
