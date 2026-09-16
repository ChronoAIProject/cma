//! Exact original message identity; current Sent evidence is deliberately separate.
use super::capabilities::NyxidCapabilities;
use layer2_core::bridge::{
    ingress::{
        AuthenticatedCallbackPayload, CallbackCapabilityExtractor, CallbackError,
        VerifiedPlatformRedelivery, VerifiedRedeliveryParent,
    },
    BridgePlatform,
};
use serde_json::Value;

pub(super) fn observe(
    payload: &AuthenticatedCallbackPayload<'_>,
) -> Result<Option<VerifiedPlatformRedelivery>, CallbackError> {
    let facts = NyxidCapabilities.observe(payload)?;
    let body: Value = serde_json::from_slice(payload.body()).map_err(|_| CallbackError::Payload)?;
    let Some(raw) = body.get("raw_platform_data") else {
        return Ok(None);
    };
    let parent = match &facts.platform {
        BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
            let Some(message) = raw.get("message") else {
                return Ok(None);
            };
            if message.get("external_reply").is_some() {
                return Ok(None);
            }
            match message.get("reply_to_message") {
                None => None,
                Some(parent) => {
                    if parent
                        .pointer("/chat/id")
                        .and_then(Value::as_i64)
                        .map(|id| id.to_string())
                        .as_deref()
                        != Some(facts.chat_id.as_str())
                    {
                        return Ok(None);
                    }
                    let Some(id) = parent.get("message_id").and_then(Value::as_i64) else {
                        return Ok(None);
                    };
                    Some(id.to_string())
                }
            }
        }
        BridgePlatform::Discord => {
            let Some(message) = raw.get("d") else {
                return Ok(None);
            };
            match message.get("message_reference") {
                None if message.get("type").and_then(Value::as_u64) == Some(0) => None,
                Some(reference)
                    if message.get("type").and_then(Value::as_u64) == Some(19)
                        && reference.get("type").is_none_or(|v| v.as_u64() == Some(0))
                        && reference.get("channel_id").and_then(Value::as_str)
                            == Some(facts.chat_id.as_str()) =>
                {
                    let Some(id) = reference.get("message_id").and_then(Value::as_str) else {
                        return Ok(None);
                    };
                    Some(id.to_owned())
                }
                _ => return Ok(None),
            }
        }
        BridgePlatform::Lark | BridgePlatform::Feishu => {
            match raw.pointer("/event/message/parent_id") {
                None => None,
                Some(value) => {
                    let Some(id) = value.as_str() else {
                        return Ok(None);
                    };
                    Some(id.to_owned())
                }
            }
        }
        BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
            super::capabilities::provider_parent(&facts.platform, raw)?
        }
    };
    if parent.as_deref()
        != payload
            .record()
            .envelope
            .reply_to_platform_message_id
            .as_deref()
    {
        return Ok(None);
    }
    let parent = parent.map_or(
        VerifiedRedeliveryParent::Absent,
        VerifiedRedeliveryParent::Message,
    );
    if let Some(attachment) = NyxidCapabilities.text_attachment(payload)? {
        VerifiedPlatformRedelivery::from_verified_text_attachment(
            payload,
            &facts,
            parent,
            &attachment,
        )
    } else {
        VerifiedPlatformRedelivery::from_verified_observation(payload, &facts, parent)
    }
}

#[cfg(test)]
mod tests;
