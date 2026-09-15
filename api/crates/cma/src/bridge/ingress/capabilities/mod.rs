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
mod telegram;

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
            BridgePlatform::Telegram => telegram::observe(raw, payload.record(), &mut facts)?,
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                lark::observe(raw, payload.record(), &mut facts)?
            }
            BridgePlatform::Discord => discord::observe(raw, payload.record(), &mut facts)?,
        }
        Ok(facts)
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

#[cfg(test)]
pub(super) mod tests;
