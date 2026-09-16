use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    if raw.get("type").and_then(Value::as_u64).is_some() {
        facts.event_kind = ChannelEventKind::InteractiveAction;
        return Ok(());
    }
    if raw.get("t").and_then(Value::as_str) == Some("MESSAGE_UPDATE") {
        facts.event_kind = ChannelEventKind::MessageEdit;
        return Ok(());
    }
    // A partial message-shaped object does not establish a create dispatch.
    if raw.get("op").and_then(Value::as_u64) != Some(0)
        || raw.get("t").and_then(Value::as_str) != Some("MESSAGE_CREATE")
    {
        return Ok(());
    }
    let message = raw.get("d").ok_or(CallbackError::Payload)?;
    let sender = message.get("author");
    let sender_id = sender.map(|sender| id(sender.get("id"))).transpose()?;
    correlate(facts, id(message.get("channel_id"))?, sender_id)?;
    facts.provider_message_id = Some(id(message.get("id"))?.into());
    let channel_type = message.pointer("/channel/type").and_then(Value::as_u64);
    facts.conversation = if message.get("guild_id").is_some_and(|v| id(Some(v)).is_ok()) {
        ChannelConversationKind::Group
    } else {
        match channel_type {
            Some(1) => ChannelConversationKind::Private,
            Some(3) => ChannelConversationKind::Group,
            _ => ChannelConversationKind::Unknown,
        }
    };
    facts.sender_kind = if sender
        .and_then(|s| s.get("system"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        ChannelSenderKind::System
    } else {
        match sender.and_then(|s| s.get("bot")).and_then(Value::as_bool) {
            Some(false) => ChannelSenderKind::Human,
            Some(true) => ChannelSenderKind::Bot,
            None => ChannelSenderKind::Unknown,
        }
    };
    facts.event_kind = if message.get("webhook_id").is_some() {
        ChannelEventKind::CallbackEcho
    } else if message
        .get("edited_timestamp")
        .is_some_and(|v| !v.is_null())
    {
        ChannelEventKind::MessageEdit
    } else if matches!(message.get("type").and_then(Value::as_u64), Some(0 | 19)) {
        message_event(
            inbox,
            message.get("content").and_then(Value::as_str),
            message
                .get("attachments")
                .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)),
        )
    } else {
        ChannelEventKind::Unknown
    };
    if facts.event_kind == ChannelEventKind::AttachmentMessage
        && crate::bridge::attachments::discord::original(raw, inbox).is_some()
    {
        facts.event_kind = ChannelEventKind::DiscordTextAttachment;
    }
    if facts.sender_kind == ChannelSenderKind::Human
        && matches!(
            facts.event_kind,
            ChannelEventKind::UserMessage | ChannelEventKind::DiscordTextAttachment
        )
    {
        facts.loop_evidence = LoopEvidence::NotLoop;
    }
    facts.thread = match channel_type {
        Some(10..=12)
            if message.pointer("/channel/id").and_then(Value::as_str)
                == Some(facts.chat_id.as_str()) =>
        {
            ThreadEvidence::Verified(
                StableThreadReference::from_verified_field(
                    StableThreadKind::DiscordChannelThread,
                    facts.chat_id.clone(),
                )
                .map_err(|_| CallbackError::Payload)?,
            )
        }
        Some(10..=12) => ThreadEvidence::Unknown,
        Some(_) => ThreadEvidence::Unsupported,
        None => ThreadEvidence::Unknown,
    };
    // message.thread may refer to a child thread created from a parent message;
    // generic thread_id can contain interaction credentials. Neither is used.
    Ok(())
}
