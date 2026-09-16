//! Polling MessageCreate events retained by the pinned X DM adapter.
use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    if raw.get("event_type").and_then(Value::as_str) != Some("MessageCreate") {
        return Ok(());
    }
    let chat = id(raw.get("dm_conversation_id"))?;
    correlate(facts, chat, Some(id(raw.get("sender_id"))?))?;
    facts.provider_message_id = Some(id(raw.get("id"))?.into());
    facts.conversation = if chat
        .split_once('-')
        .is_some_and(|(a, b)| numeric(a) && numeric(b))
    {
        ChannelConversationKind::Private
    } else if numeric(chat) {
        ChannelConversationKind::Group
    } else {
        ChannelConversationKind::Unknown
    };
    facts.thread = ThreadEvidence::Unsupported;
    facts.event_kind = message_event(
        inbox,
        raw.get("text").and_then(Value::as_str),
        raw.get("attachments").is_some_and(|attachments| {
            !attachments
                .get("media_keys")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        }),
    );
    // Upstream filters its own account during polling, but the signed raw event
    // does not carry that identity or a human/bot marker. Keep both facts unknown.
    Ok(())
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_digit())
}

pub(super) fn parent(raw: &Value) -> Result<Option<String>, CallbackError> {
    let Some(references) = raw.get("referenced_events") else {
        return Ok(None);
    };
    let events = references.as_array().ok_or(CallbackError::Payload)?;
    events
        .iter()
        .find(|event| event.get("type").and_then(Value::as_str) == Some("replied_to"))
        .map(|event| id(event.get("id")).map(str::to_owned))
        .transpose()
}
