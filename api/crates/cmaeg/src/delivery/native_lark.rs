//! Text replies through the retained app/chat, with durable confirmed target receipts.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    native_lark::{
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
        if message.validate().is_err()
            || !message.native_attachments.is_empty()
            || crate::interactions::has_controls(message)
            || message.whatsapp_template.is_some()
            || message.reply_to.is_some()
            || message.thread_reply
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
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_lark());
        let mut result = IntentReceipt {
            rendering,
            ..rejected("empty_message")
        };
        let route = format!(
            "native-lark:{}:{}:{}:{}",
            reply.identity.region.as_str(),
            reply.identity.app_id,
            reply.identity.tenant_key,
            reply.chat_id
        );
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(t) => t,
            Err(r) => {
                result.delivery = r.outcome();
                return (result, effects);
            }
        };
        let mut confirmed = 0;
        let mut first = None;
        for part in &parts {
            let Some(_pace) = self.pacing.acquire(&route).await else {
                result = rejected("pacing_capacity");
                break;
            };
            let token = match self
                .relay
                .native_lark
                .token(&reply.identity, credentials)
                .await
            {
                Ok(token) => token,
                Err(_) => {
                    result = rejected("native_lark_credentials_unavailable");
                    break;
                }
            };
            let permit = match ctx.gate.attempt().await {
                Ok(p) => p,
                Err(r) => {
                    result.delivery = r.outcome();
                    break;
                }
            };
            if !ctx.gate.commit() {
                result.delivery = super::gate::expired();
                break;
            }
            let sent = self
                .relay
                .native_lark
                .send_text(&token, reply, &part.text)
                .await;
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(id) => {
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
                    if store.put(&receipt).await.is_err() {
                        result.delivery = DeliveryOutcome::Unknown {
                            reason: "native_lark_receipt_unconfirmed".into(),
                        };
                        break;
                    }
                    if first.is_none() {
                        first = Some(id)
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
        if confirmed > 0 && confirmed < parts.len() {
            result.delivery = DeliveryOutcome::Unknown {
                reason: "native_lark_partial_delivery".into(),
            }
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
