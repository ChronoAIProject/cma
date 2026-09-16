//! Cloud API message objects, not the unsigned top-level webhook or media bytes.
use layer2_core::bridge::ingress::StoredInbox;

use super::*;

pub(super) fn observe(
    raw: &Value,
    inbox: &StoredInbox,
    facts: &mut PlatformCapabilityObservation,
) -> Result<(), CallbackError> {
    let kind = raw.get("type").and_then(Value::as_str);
    if matches!(kind, Some("button" | "interactive")) {
        facts.event_kind = ChannelEventKind::InteractiveAction;
        return Ok(());
    }
    if !matches!(
        kind,
        Some(
            "text"
                | "image"
                | "audio"
                | "video"
                | "document"
                | "sticker"
                | "location"
                | "contacts"
                | "order"
        )
    ) {
        return Ok(());
    }
    let sender = id(raw.get("from"))?;
    correlate(facts, sender, Some(sender))?;
    facts.provider_message_id = Some(id(raw.get("id"))?.into());
    facts.conversation = ChannelConversationKind::Private;
    facts.thread = ThreadEvidence::Unsupported;
    facts.event_kind = if matches!(kind, Some("location" | "contacts" | "order")) {
        ChannelEventKind::UnsupportedContent
    } else {
        message_event(
            inbox,
            (kind == Some("text"))
                .then(|| raw.pointer("/text/body").and_then(Value::as_str))
                .flatten(),
            matches!(
                kind,
                Some("image" | "audio" | "video" | "document" | "sticker")
            ),
        )
    };
    // The signed message has a stable phone identity but no human/bot marker.
    // Graph media lookup URLs never establish credential or download authority.
    Ok(())
}

pub(super) fn parent(raw: &Value) -> Result<Option<String>, CallbackError> {
    raw.pointer("/context/id")
        .map(|value| id(Some(value)).map(str::to_owned))
        .transpose()
}
