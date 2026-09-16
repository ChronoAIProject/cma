//! Pinned event_callback fields. A user ID alone does not prove a human sender.
use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    if raw.get("type").and_then(Value::as_str) != Some("event_callback") {
        return Ok(());
    }
    let event = raw.get("event").ok_or(CallbackError::Payload)?;
    if !matches!(
        event.get("type").and_then(Value::as_str),
        Some("message" | "app_mention")
    ) {
        return Ok(());
    }
    let subtype = event.get("subtype").and_then(Value::as_str);
    if subtype == Some("message_changed") {
        facts.event_kind = ChannelEventKind::MessageEdit;
        return Ok(());
    }
    if event.get("bot_id").is_some() || subtype == Some("bot_message") {
        facts.sender_kind = ChannelSenderKind::Bot;
        facts.loop_evidence = LoopEvidence::Loop;
        return Ok(());
    }
    if subtype.is_some_and(|kind| !matches!(kind, "file_share" | "thread_broadcast")) {
        return Ok(());
    }
    let chat = id(event.get("channel"))?;
    correlate(facts, chat, Some(id(event.get("user"))?))?;
    facts.provider_message_id = Some(id(event.get("ts"))?.into());
    facts.conversation = match chat.chars().next() {
        Some('D') => ChannelConversationKind::Private,
        Some('C') => ChannelConversationKind::Channel,
        Some('G') => ChannelConversationKind::Group,
        _ => ChannelConversationKind::Unknown,
    };
    facts.event_kind = message_event(
        inbox,
        event.get("text").and_then(Value::as_str),
        event
            .get("files")
            .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)),
    );
    facts.thread = match event.get("thread_ts") {
        None => ThreadEvidence::Missing,
        Some(value) => ThreadEvidence::Verified(
            StableThreadReference::from_verified_field(
                StableThreadKind::SlackThread,
                id(Some(value))?.into(),
            )
            .map_err(|_| CallbackError::Payload)?,
        ),
    };
    // Pinned callbacks preserve the event, but provide no explicit human or
    // selected-bot identity observation. Absence of bot_id is not that evidence.
    Ok(())
}

pub(super) fn parent(raw: &Value) -> Result<Option<String>, CallbackError> {
    let event = raw.get("event").ok_or(CallbackError::Payload)?;
    let Some(value) = event.get("thread_ts") else {
        return Ok(None);
    };
    let thread = id(Some(value))?;
    Ok((thread != id(event.get("ts"))?).then(|| thread.into()))
}
