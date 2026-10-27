//! Native text replies use the same bound Meta phone and persist exact send targets.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    tools::Capabilities,
    whatsapp::{
        receipts::{Receipt, RETENTION},
        Error,
    },
};
use cmaeg_core::{
    activity::SourceKind,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn whatsapp_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        // Native sends do not consume or change a relay acknowledgement token.
        let effects = SendEffects::default();
        let ReplyAddress::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            recipient_id,
            message_id,
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        if !message.native_attachments.is_empty()
            || message.whatsapp_template.is_some()
            || message.thread_reply
            || message.validate().is_err()
        {
            return (unsupported(), effects);
        }
        let Some(credentials) = crate::whatsapp::credentials(&ctx.channel.definition, &event.reply)
        else {
            return (rejected("native_whatsapp_destination_revoked"), effects);
        };
        let Some(store) = &self.relay.whatsapp_receipts else {
            return (
                rejected("native_whatsapp_receipt_storage_unavailable"),
                effects,
            );
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return (rejected("native_whatsapp_consumer_unavailable"), effects);
        };
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_whatsapp());
        let mut result = IntentReceipt {
            rendering,
            ..rejected("empty_message")
        };
        let route = format!("whatsapp:{app_id}:{waba_id}:{phone_number_id}:{recipient_id}");
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(refusal) => {
                result.delivery = refusal.outcome();
                return (result, effects);
            }
        };
        let mut confirmed = 0;
        let mut first = None;
        for part in parts {
            let Some(_pace) = self.pacing.acquire(&route).await else {
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let permit = match ctx.gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => {
                    result.delivery = refusal.outcome();
                    break;
                }
            };
            if !ctx.gate.commit() {
                result.delivery = super::gate::expired();
                break;
            }
            let sent = self
                .relay
                .whatsapp
                .send_text(
                    credentials,
                    phone_number_id,
                    recipient_id,
                    message_id,
                    &part.text,
                )
                .await;
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(platform_id) => {
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    let receipt = Receipt {
                        kind: "native_whatsapp_receipt_v1".into(),
                        original: EventIdentity {
                            owner: ctx.channel.definition.owner.subject.clone(),
                            channel_id: event.channel_id.clone(),
                            record_id: event.record_id.clone(),
                            event_id: event.event_id.clone(),
                        },
                        source: SourceKind::Whatsapp {
                            app_id: app_id.clone(),
                            waba_id: waba_id.clone(),
                            phone_number_id: phone_number_id.clone(),
                        },
                        recipient: recipient_id.clone(),
                        message_id: platform_id.clone(),
                        consumer: consumer.id.clone(),
                        provider: consumer.provider.clone(),
                        partition_key: event.partition_key.clone(),
                        conversation_id: event.conversation_id.clone(),
                        confirmed_at: now,
                        retained_until: now + RETENTION,
                    };
                    confirmed += 1;
                    if store.put(&receipt).await.is_err() {
                        result.delivery = DeliveryOutcome::Unknown {
                            reason: "native_whatsapp_receipt_unconfirmed".into(),
                        };
                        break;
                    }
                    if first.is_none() {
                        first = Some(platform_id);
                    }
                    result.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                }
                Err(error) => {
                    result.delivery = match error {
                        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
                        Error::Rejected | Error::Invalid | Error::CustodyUnproved => {
                            DeliveryOutcome::Rejected {
                                reason: "native_whatsapp_send_rejected".into(),
                            }
                        }
                        Error::Unconfirmed => DeliveryOutcome::Unknown {
                            reason: "native_whatsapp_acceptance_unconfirmed".into(),
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
                reason: "native_whatsapp_partial_delivery".into(),
            };
        }
        (result, effects)
    }
}
fn rejected(reason: &'static str) -> IntentReceipt {
    let mut result = unsupported();
    result.delivery = DeliveryOutcome::Rejected {
        reason: reason.into(),
    };
    result
}
