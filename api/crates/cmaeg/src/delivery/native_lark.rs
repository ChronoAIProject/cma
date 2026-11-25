//! Text and bounded media replies through the retained app/chat, with durable confirmed target receipts.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    render::{LiteralText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    native_lark::{
        outbound::{MediaUpload, UploadedMedia},
        receipts::{Receipt, RETENTION},
        Error,
    },
    tools::Capabilities,
};
use cmaeg_core::{
    activity::SourceKind,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
use std::{collections::VecDeque, time::Duration};
use tokio::time::{timeout_at, Instant};

enum NativePart {
    Text(String),
    Upload(MediaUpload),
    Media(UploadedMedia),
    ReceivedSticker(crate::native_lark::stickers::RetainedSticker),
}
impl Sender {
    pub(super) async fn native_lark_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::NativeLark(reply) = &event.reply else {
            return (unsupported(), effects);
        };
        if message.thread_reply && reply.thread_id.is_none() {
            return self.native_lark_thread_start(ctx, event, message).await;
        }
        if self.relay.native_lark_controls
            && (crate::interactions::lark::requested(message)
                || crate::interactions::lark_form::requested(message))
        {
            return self.native_lark_controls_reply(ctx, event, message).await;
        }
        let media = !message.native_attachments.is_empty();
        let sticker = message.reply_with_received_sticker;
        if message.validate().is_err()
            || (media && !self.relay.native_lark_outbound_media_enabled())
            || (sticker && !self.relay.native_lark_sticker_replies)
            || crate::interactions::has_controls(message)
            || message.whatsapp_template.is_some()
            || message.reply_to.is_some()
        {
            return (unsupported(), effects);
        }
        let Some(credentials) =
            crate::native_lark::credentials(&ctx.channel.definition, &event.reply)
        else {
            return (rejected("native_lark_destination_revoked"), effects);
        };
        let Some(store) = &self.relay.native_lark_receipts else {
            return (rejected("native_lark_receipts_unavailable"), effects);
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return (rejected("native_lark_consumer_unavailable"), effects);
        };
        let (parts, rendering) = LiteralText.render(message, &Capabilities::native_lark());
        let editable = !media && !sticker && parts.len() == 1;
        let mut result = IntentReceipt {
            message_parts: vec![],
            rendering,
            ..rejected("empty_message")
        };
        let deadline = ctx
            .gate
            .transport_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
        let prepared = async {
            if sticker {
                let records = self.relay.interactions.as_ref()?.channel_records();
                let proved = crate::native_lark::stickers::load(
                    records,
                    self.keys.clone(),
                    ctx.channel,
                    event,
                )
                .await?;
                return Some(VecDeque::from([NativePart::ReceivedSticker(proved)]));
            }
            let files = crate::media::outbound::prepare(&message.native_attachments)
                .await
                .ok()?;
            let mut operations: VecDeque<_> = parts
                .into_iter()
                .map(|part| NativePart::Text(part.text))
                .collect();
            for (item, file) in message.native_attachments.iter().zip(files) {
                operations.push_back(NativePart::Upload(
                    crate::native_lark::Client::prepare_media(item, file)
                        .await
                        .ok()?,
                ));
            }
            Some(operations)
        };
        let Ok(Some(mut operations)) = timeout_at(deadline, prepared).await else {
            result.delivery = rejected("native_lark_media_invalid").delivery;
            return (result, effects);
        };
        let route = format!(
            "native-lark:{}:{}:{}:{}",
            reply.identity.region.as_str(),
            reply.identity.app_id,
            reply.identity.tenant_key,
            reply.chat_id
        );
        let _ticket = match timeout_at(deadline, ctx.gate.enter_route(&route)).await {
            Ok(Ok(t)) => t,
            Ok(Err(r)) => {
                result.delivery = r.outcome();
                return (result, effects);
            }
            Err(_) => {
                result.delivery = super::gate::expired();
                return (result, effects);
            }
        };
        let mut confirmed = 0;
        let mut first = None;
        let mut retained = None;
        while let Some(operation) = operations.pop_front() {
            let Ok(Some(_pace)) = timeout_at(deadline, self.pacing.acquire(&route)).await else {
                result.delivery = rejected("pacing_capacity").delivery;
                break;
            };
            let token = timeout_at(deadline, async {
                if media || sticker || self.relay.native_lark_effects {
                    self.relay
                        .native_lark
                        .verified_token(&reply.identity, credentials)
                        .await
                } else {
                    self.relay
                        .native_lark
                        .token(&reply.identity, credentials)
                        .await
                }
            })
            .await;
            let Ok(Ok(token)) = token else {
                result.delivery = rejected("native_lark_credentials_unavailable").delivery;
                break;
            };
            // Check current default/named consumer, provider and source revision after
            // custody verification and before each upload or recipient message write.
            let permit = match timeout_at(deadline, ctx.gate.attempt()).await {
                Ok(Ok(p)) => p,
                Ok(Err(r)) => {
                    result.delivery = r.outcome();
                    break;
                }
                Err(_) => {
                    result.delivery = super::gate::expired();
                    break;
                }
            };
            if !ctx.gate.commit() {
                result.delivery = super::gate::expired();
                break;
            }
            let upload = matches!(&operation, NativePart::Upload(_));
            let send = async {
                match operation {
                    NativePart::ReceivedSticker(proved) => self
                        .relay
                        .native_lark
                        .send_received_sticker(&token, proved)
                        .await
                        .map(Some),
                    NativePart::Text(text) => self
                        .relay
                        .native_lark
                        .send_text(&token, reply, &text)
                        .await
                        .map(Some),
                    NativePart::Upload(upload) => {
                        let uploaded = self
                            .relay
                            .native_lark
                            .upload_media(&token, reply, upload)
                            .await?;
                        operations.push_front(NativePart::Media(uploaded));
                        Ok(None)
                    }
                    NativePart::Media(uploaded) => self
                        .relay
                        .native_lark
                        .send_media(&token, reply, uploaded)
                        .await
                        .map(Some),
                }
            };
            let sent = super::trace::native::part(
                confirmed,
                upload,
                async {
                    timeout_at(deadline, send)
                        .await
                        .unwrap_or(Err(Error::Unconfirmed))
                },
                super::trace::native::lark,
            )
            .await;
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(None) => continue,
                Ok(Some(id)) => {
                    confirmed += 1;
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    let mut destination = reply.clone();
                    destination.message_id = id.clone();
                    let receipt = Receipt {
                        kind: "native_lark_receipt_v1".into(),
                        original: EventIdentity {
                            owner: ctx.channel.definition.owner.subject.clone(),
                            channel_id: event.channel_id.clone(),
                            record_id: event.record_id.clone(),
                            event_id: event.event_id.clone(),
                        },
                        source: SourceKind::NativeLark(reply.identity.clone()),
                        recipient: reply.chat_id.clone(),
                        reply: destination,
                        message_id: id.clone(),
                        consumer: consumer.id.clone(),
                        provider: consumer.provider.clone(),
                        partition_key: event.partition_key.clone(),
                        conversation_id: event.conversation_id.clone(),
                        confirmed_at: now,
                        retained_until: now + RETENTION,
                    };
                    if !super::gate::within_deadline(
                        ctx.gate,
                        async { store.put(&receipt).await.is_ok() },
                        false,
                    )
                    .await
                    {
                        result.delivery = DeliveryOutcome::Unknown {
                            reason: "native_lark_receipt_unconfirmed".into(),
                        };
                        break;
                    }
                    if first.is_none() {
                        first = Some(id);
                        retained = Some(receipt);
                    }
                    result.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                }
                Err(error) => {
                    result.delivery = match error {
                        Error::Invalid | Error::Rejected => DeliveryOutcome::Rejected {
                            reason: "native_lark_send_rejected".into(),
                        },
                        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
                        Error::Unconfirmed => DeliveryOutcome::Unknown {
                            reason: "native_lark_acceptance_unconfirmed".into(),
                        },
                    };
                    break;
                }
            }
        }
        if confirmed > 0
            && !matches!(
                result.delivery,
                DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
            )
        {
            result.delivery = DeliveryOutcome::Unknown {
                reason: "native_lark_partial_delivery".into(),
            }
        }
        if self.relay.native_lark_effects
            && confirmed == 1
            && matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
        {
            result.message_ref = retained.as_ref().and_then(|receipt| {
                crate::tools::message_effects::native_lark_authority::confirmed(
                    ctx.channel,
                    receipt,
                    editable,
                    &self.keys,
                )
            });
        }
        (result, effects)
    }
}
fn rejected(reason: &str) -> IntentReceipt {
    let mut result = unsupported();
    result.delivery = DeliveryOutcome::Rejected {
        reason: reason.into(),
    };
    result
}
