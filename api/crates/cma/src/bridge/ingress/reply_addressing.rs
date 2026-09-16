//! Positive reply addressing comes from a permanent, confirmed CMA send in the
//! same actual chat. Signed text and generic bot/name hints never supply it.
use layer2_core::bridge::{
    ingress::{AuthenticatedCallbackPayload, CallbackCapabilityExtractor, CallbackError},
    outbox::{DeliveryState, OutboxItem},
    partition::{
        CapabilityContract, ChannelEventKind, ChannelSenderKind, ConfirmedReplyReceipt,
        LoopEvidence, PartitionSourcePin, PlatformCapabilityObservation, VerifiedFlag,
    },
    BridgePlatform,
};
use layer2_core::resource_access::ResourceOwner;
use serde_json::Value;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ReplyLookup {
    pub source: PartitionSourcePin,
    pub owner: ResourceOwner,
    pub issuer: String,
    pub bot_id: String,
    pub platform: BridgePlatform,
    pub chat_id: String,
    pub parent_id: String,
}

impl ReplyLookup {
    pub(crate) fn authenticated(
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<Option<Self>, CallbackError> {
        let facts = super::capabilities::NyxidCapabilities.observe(payload)?;
        if facts.sender_kind != ChannelSenderKind::Human
            || !matches!(
                facts.event_kind,
                ChannelEventKind::UserMessage | ChannelEventKind::DiscordTextAttachment
            )
            || facts.loop_evidence != LoopEvidence::NotLoop
        {
            return Ok(None);
        }
        let record = payload.record();
        let Some(parent) = record.envelope.reply_to_platform_message_id.as_ref() else {
            return Ok(None);
        };
        if parent.is_empty() || parent.len() > 256 || parent.chars().any(char::is_control) {
            return Err(CallbackError::Payload);
        }
        let body: Value =
            serde_json::from_slice(payload.body()).map_err(|_| CallbackError::Payload)?;
        let Some(raw) = body.get("raw_platform_data") else {
            return Ok(None);
        };
        let provider_parent = match facts.platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                let Some(reply) = raw.pointer("/message/reply_to_message") else {
                    return Ok(None);
                };
                // Telegram reply-to Message carries its own Chat. A forwarded
                // or external-chat object cannot corroborate this chat's reply.
                if reply
                    .pointer("/chat/id")
                    .and_then(Value::as_i64)
                    .map(|id| id.to_string())
                    .as_deref()
                    != Some(facts.chat_id.as_str())
                {
                    return Ok(None);
                }
                reply
                    .get("message_id")
                    .and_then(Value::as_i64)
                    .map(|id| id.to_string())
            }
            BridgePlatform::Discord => {
                let Some(reference) = raw.pointer("/d/message_reference") else {
                    return Ok(None);
                };
                // Forward/crosspost references are not replies. The actual
                // message and reference must both carry the same channel ID.
                if raw.pointer("/d/type").and_then(Value::as_u64) != Some(19)
                    || reference
                        .get("type")
                        .is_some_and(|kind| kind.as_u64() != Some(0))
                    || reference.get("channel_id").and_then(Value::as_str)
                        != Some(facts.chat_id.as_str())
                {
                    return Ok(None);
                }
                reference
                    .get("message_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => raw
                .pointer("/event/message/parent_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
                super::capabilities::provider_parent(&facts.platform, raw)?
            }
        };
        if provider_parent.as_ref() != Some(parent) {
            return Ok(None);
        }
        Ok(Some(Self {
            source: facts.source,
            owner: record.owner.clone(),
            issuer: record.attestation.issuer.clone(),
            bot_id: record.bot_id.clone(),
            platform: facts.platform,
            chat_id: facts.chat_id,
            parent_id: parent.clone(),
        }))
    }

    pub(crate) fn retained(
        record: &layer2_core::bridge::ingress::StoredInbox,
    ) -> Result<Self, CallbackError> {
        Ok(Self {
            source: PartitionSourcePin::for_inbox(record).map_err(|_| CallbackError::Payload)?,
            owner: record.owner.clone(),
            issuer: record.attestation.issuer.clone(),
            bot_id: record.bot_id.clone(),
            platform: serde_json::from_value(Value::String(record.envelope.platform.clone()))
                .map_err(|_| CallbackError::Payload)?,
            chat_id: record.envelope.chat_id.clone(),
            parent_id: record
                .envelope
                .reply_to_platform_message_id
                .clone()
                .ok_or(CallbackError::Payload)?,
        })
    }

    pub(crate) fn confirm(
        &self,
        item: &OutboxItem,
    ) -> Result<Option<ConfirmedReply>, CallbackError> {
        item.validate().map_err(|_| CallbackError::Unavailable)?;
        let DeliveryState::Sent { receipt } = &item.state else {
            return Ok(None);
        };
        let source = &item.source;
        let connector = &source.config().connector;
        if receipt.platform_message_id.as_ref() != Some(&self.parent_id)
            || source.owner() != &self.owner
            || source.pin().bridge_id != self.source.work_pin().bridge_id
            || connector.issuer != self.issuer
            || connector.bot_id != self.bot_id
            || connector.platform != self.platform
            || source.chat_id() != self.chat_id
        {
            return Ok(None);
        }
        Ok(Some(ConfirmedReply {
            lookup: self.clone(),
            receipt: ConfirmedReplyReceipt {
                item_id: item.id.clone(),
                revision: item.revision,
                record_sha256: layer2_core::profiles::content_digest(item)
                    .map_err(|_| CallbackError::Unavailable)?,
            },
        }))
    }
}

pub(crate) struct ConfirmedReply {
    lookup: ReplyLookup,
    receipt: ConfirmedReplyReceipt,
}

impl ConfirmedReply {
    pub(crate) fn receipt(&self) -> &ConfirmedReplyReceipt {
        &self.receipt
    }
}

#[async_trait::async_trait]
pub(crate) trait ReplyAddressingStore: Send + Sync {
    async fn confirmed_reply(
        &self,
        lookup: &ReplyLookup,
    ) -> Result<Option<ConfirmedReply>, CallbackError>;
}

pub(crate) struct ReplyCapabilities(pub Option<ConfirmedReply>);

impl CallbackCapabilityExtractor for ReplyCapabilities {
    fn contract(&self) -> CapabilityContract {
        CapabilityContract::NyxidPlatformV1
    }
    fn text_attachment(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<Option<layer2_core::bridge::ingress::VerifiedTextAttachment>, CallbackError> {
        super::capabilities::NyxidCapabilities.text_attachment(payload)
    }
    fn observe(
        &self,
        payload: &AuthenticatedCallbackPayload<'_>,
    ) -> Result<PlatformCapabilityObservation, CallbackError> {
        let mut facts = super::capabilities::NyxidCapabilities.observe(payload)?;
        if let Some(proof) = &self.0 {
            if ReplyLookup::authenticated(payload)?.as_ref() != Some(&proof.lookup) {
                return Err(CallbackError::Payload);
            }
            facts.replied_to_bot = VerifiedFlag::Yes;
            facts.reply_receipt = Some(proof.receipt.clone());
        }
        Ok(facts)
    }
}

#[cfg(test)]
pub(crate) mod tests;
