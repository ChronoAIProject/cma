use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    let event_type = raw.pointer("/header/event_type").and_then(Value::as_str);
    if event_type == Some("card.action.trigger") {
        facts.event_kind = ChannelEventKind::InteractiveAction;
        return Ok(());
    }
    if raw.get("schema").and_then(Value::as_str) != Some("2.0")
        || event_type != Some("im.message.receive_v1")
    {
        return Ok(());
    }
    let message = raw
        .pointer("/event/message")
        .ok_or(CallbackError::Payload)?;
    let sender = raw.pointer("/event/sender");
    let chat = id(message.get("chat_id"))?;
    let sender_id = sender
        .map(|sender| id(sender.pointer("/sender_id/open_id")))
        .transpose()?;
    correlate(facts, chat, sender_id)?;
    facts.provider_message_id = Some(id(message.get("message_id"))?.into());
    facts.conversation = match message.get("chat_type").and_then(Value::as_str) {
        Some("p2p") => ChannelConversationKind::Private,
        Some("group") => ChannelConversationKind::Group,
        _ => ChannelConversationKind::Unknown,
    };
    facts.sender_kind = match sender
        .and_then(|s| s.get("sender_type"))
        .and_then(Value::as_str)
    {
        Some("user") => ChannelSenderKind::Human,
        Some("app") => ChannelSenderKind::Bot,
        _ => ChannelSenderKind::Unknown,
    };
    let message_type = message.get("message_type").and_then(Value::as_str);
    let content: Option<Value> = message
        .get("content")
        .and_then(Value::as_str)
        .and_then(|content| serde_json::from_str(content).ok());
    facts.event_kind = message_event(
        inbox,
        (message_type == Some("text"))
            .then(|| {
                content
                    .as_ref()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
            })
            .flatten(),
        matches!(
            message_type,
            Some("image" | "file" | "audio" | "media" | "sticker")
        ),
    );
    if facts.sender_kind == ChannelSenderKind::Human
        && facts.event_kind == ChannelEventKind::UserMessage
    {
        facts.loop_evidence = LoopEvidence::NotLoop;
    }
    facts.thread = match message.get("thread_id").filter(|v| !v.is_null()) {
        None => ThreadEvidence::Missing,
        Some(value) => ThreadEvidence::Verified(
            StableThreadReference::from_verified_field(
                if facts.platform == BridgePlatform::Lark {
                    StableThreadKind::LarkThread
                } else {
                    StableThreadKind::FeishuThread
                },
                id(Some(value))?.into(),
            )
            .map_err(|_| CallbackError::Payload)?,
        ),
    };
    // NyxID's Lark platform_bot_id is an app ID, while mentions use open_id.
    // A parent message ID also does not establish that parent's selected bot.
    Ok(())
}
