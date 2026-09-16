//! Facts from NyxID's signed raw platform payload, never callback authority.
//! The contract is source/fixture verified; live platform receipts are separate.
use layer2_core::bridge::ingress::{
    AuthenticatedCallbackPayload, CallbackCapabilityExtractor, CallbackError,
};
use layer2_core::bridge::partition::*;
use layer2_core::bridge::BridgePlatform;
use serde_json::Value;

mod discord;
mod lark;
mod slack;
mod telegram;
mod whatsapp;
mod x;

pub(crate) struct NyxidCapabilities;

impl CallbackCapabilityExtractor for NyxidCapabilities {
    fn contract(&self) -> CapabilityContract {
        CapabilityContract::NyxidPlatformV1
    }

    fn observe(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<PlatformCapabilityObservation, CallbackError> {
        let mut facts = PlatformCapabilityObservation::unknown(payload.record())
            .map_err(|_| CallbackError::Payload)?;
        // The generic envelope's conversation type describes a route, which
        // may be a wildcard. Only actual platform metadata establishes it.
        facts.conversation = ChannelConversationKind::Unknown;
        let callback: Value =
            serde_json::from_slice(payload.body()).map_err(|_| CallbackError::Payload)?;
        let Some(raw) = callback.get("raw_platform_data").filter(|v| v.is_object()) else {
            return Ok(facts);
        };
        match facts.platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                telegram::observe(raw, payload.record(), &mut facts)?
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                lark::observe(raw, payload.record(), &mut facts)?
            }
            BridgePlatform::Discord => discord::observe(raw, payload.record(), &mut facts)?,
            BridgePlatform::Slack => slack::observe(raw, payload.record(), &mut facts)?,
            BridgePlatform::Whatsapp => whatsapp::observe(raw, payload.record(), &mut facts)?,
            BridgePlatform::X => x::observe(raw, payload.record(), &mut facts)?,
        }
        Ok(facts)
    }
}

pub(super) fn provider_parent(
    platform: &BridgePlatform,
    raw: &Value,
) -> Result<Option<String>, CallbackError> {
    match platform {
        BridgePlatform::Slack => slack::parent(raw),
        BridgePlatform::Whatsapp => whatsapp::parent(raw),
        BridgePlatform::X => x::parent(raw),
        _ => Ok(None),
    }
}

fn id(value: Option<&Value>) -> Result<&str, CallbackError> {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .ok_or(CallbackError::Payload)
}

fn correlate(
    facts: &PlatformCapabilityObservation,
    chat: &str,
    sender: Option<&str>,
) -> Result<(), CallbackError> {
    if facts.chat_id != chat || sender.is_some_and(|sender| sender != facts.sender_id) {
        return Err(CallbackError::Payload);
    }
    Ok(())
}

fn text_matches(record: &layer2_core::bridge::ingress::StoredInbox, text: Option<&str>) -> bool {
    record.envelope.content_type == "text"
        && record.envelope.attachments.is_empty()
        && text.is_some()
        && record.envelope.text.as_deref() == text
}

/// Refuse whole messages with media; never silently discard their attachments.
/// This runs only after the platform established an original message identity.
fn message_event(
    record: &layer2_core::bridge::ingress::StoredInbox,
    text: Option<&str>,
    raw_media: bool,
) -> ChannelEventKind {
    if raw_media || !record.envelope.attachments.is_empty() {
        ChannelEventKind::AttachmentMessage
    } else if record.envelope.content_type != "text" {
        ChannelEventKind::UnsupportedContent
    } else if text_matches(record, text) {
        ChannelEventKind::UserMessage
    } else {
        ChannelEventKind::Unknown
    }
}

#[cfg(test)]
pub(super) mod tests;
