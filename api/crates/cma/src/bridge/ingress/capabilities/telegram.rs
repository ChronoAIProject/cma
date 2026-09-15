use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    let events: Vec<_> = [
        "message",
        "edited_message",
        "channel_post",
        "edited_channel_post",
    ]
    .into_iter()
    .filter_map(|kind| raw.get(kind).map(|value| (kind, value)))
    .collect();
    if events.len() > 1 {
        return Err(CallbackError::Payload);
    }
    let Some((kind, message)) = events.first() else {
        if raw.get("callback_query").is_some() {
            facts.event_kind = ChannelEventKind::InteractiveAction;
        }
        return Ok(());
    };
    let chat = message.get("chat").ok_or(CallbackError::Payload)?;
    let chat_id = numeric(chat.get("id"))?;
    let sender = message.get("from");
    let sender_id = sender.map(|sender| numeric(sender.get("id"))).transpose()?;
    correlate(facts, &chat_id, sender_id.as_deref())?;
    facts.provider_message_id = Some(numeric(message.get("message_id"))?);
    facts.conversation = match chat.get("type").and_then(Value::as_str) {
        Some("private") => ChannelConversationKind::Private,
        Some("group" | "supergroup") => ChannelConversationKind::Group,
        Some("channel") => ChannelConversationKind::Channel,
        _ => ChannelConversationKind::Unknown,
    };
    facts.sender_kind = if message.get("sender_chat").is_some() {
        ChannelSenderKind::System
    } else {
        match sender
            .and_then(|s| s.get("is_bot"))
            .and_then(Value::as_bool)
        {
            Some(false) => ChannelSenderKind::Human,
            Some(true) => ChannelSenderKind::Bot,
            None => ChannelSenderKind::Unknown,
        }
    };
    facts.event_kind = if kind.starts_with("edited_") {
        ChannelEventKind::MessageEdit
    } else if text_matches(inbox, message.get("text").and_then(Value::as_str)) {
        ChannelEventKind::UserMessage
    } else if [
        "new_chat_members",
        "left_chat_member",
        "new_chat_title",
        "pinned_message",
    ]
    .iter()
    .any(|field| message.get(*field).is_some())
    {
        ChannelEventKind::SystemEvent
    } else {
        ChannelEventKind::Unknown
    };
    facts.loop_evidence = if message.get("via_bot").is_some() {
        LoopEvidence::Loop
    } else if facts.sender_kind == ChannelSenderKind::Human
        && facts.event_kind == ChannelEventKind::UserMessage
    {
        LoopEvidence::NotLoop
    } else {
        LoopEvidence::Unknown
    };
    facts.thread = match message.get("message_thread_id") {
        Some(thread)
            if message.get("is_topic_message").and_then(Value::as_bool) == Some(true)
                && facts.conversation == ChannelConversationKind::Group =>
        {
            ThreadEvidence::Verified(
                StableThreadReference::from_verified_field(
                    StableThreadKind::TelegramForumTopic,
                    numeric(Some(thread))?,
                )
                .map_err(|_| CallbackError::Payload)?,
            )
        }
        Some(_) => ThreadEvidence::Unknown,
        None if facts.conversation == ChannelConversationKind::Private => {
            ThreadEvidence::Unsupported
        }
        None => ThreadEvidence::Missing,
    };
    // The selected NyxID bot UUID is not a Telegram user ID. Neither generic
    // @username text nor a reply to any bot proves addressing this selected bot.
    Ok(())
}

fn numeric(value: Option<&Value>) -> Result<String, CallbackError> {
    // Telegram's documented integer wire field is normalized exactly as NyxID
    // does. The result must equal the signed opaque ID; no cross-ID coercion.
    value
        .and_then(Value::as_i64)
        .map(|n| n.to_string())
        .ok_or(CallbackError::Payload)
}
