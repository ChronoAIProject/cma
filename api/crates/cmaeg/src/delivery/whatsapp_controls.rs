//! Native controls persist their card before sending and bind only confirmed receipts.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    interactions,
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
    pub(super) async fn whatsapp_controls_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
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
        if message.reply_to.is_some()
            || !self.relay.native_whatsapp_interactions_enabled()
            || (event.consumer.is_some() && !self.relay.consumer_interactions_enabled())
        {
            return (unsupported(), effects);
        }
        let Ok((_, rendering)) =
            interactions::whatsapp::render(message, "00000000000000000000000000000000", 1)
        else {
            return (unsupported(), effects);
        };
        let rejected = |reason: &str| {
            refused(
                rendering.clone(),
                DeliveryOutcome::Rejected {
                    reason: reason.into(),
                },
            )
        };
        let Some(credentials) = crate::whatsapp::credentials(&ctx.channel.definition, &event.reply)
            .filter(|credentials| credentials.validate().is_ok())
        else {
            return (rejected("native_whatsapp_destination_revoked"), effects);
        };
        let (Some(cards), Some(receipts)) =
            (&self.relay.interactions, &self.relay.whatsapp_receipts)
        else {
            return (
                rejected("native_whatsapp_control_storage_unavailable"),
                effects,
            );
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return (rejected("native_whatsapp_consumer_unavailable"), effects);
        };
        let context = match cards.event_context(ctx.channel, event).await {
            Ok(context)
                if interactions::whatsapp::eligible(ctx.channel, event, &context.activity) =>
            {
                context
            }
            _ => {
                return (
                    rejected("native_whatsapp_control_context_unavailable"),
                    effects,
                )
            }
        };
        let route = format!("whatsapp:{app_id}:{waba_id}:{phone_number_id}:{recipient_id}");
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(error) => return (refused(rendering, error.outcome()), effects),
        };
        // Durable card creation is the first side effect. It has no destination
        // authority until the exact native response and receipt are persisted.
        if !ctx.gate.commit() {
            return (refused(rendering, super::gate::expired()), effects);
        }
        let mut prepared = match cards
            .prepare_with_context(ctx.channel, event, message, context)
            .await
        {
            Ok(prepared) => prepared,
            Err(_) => {
                return (
                    rejected("native_whatsapp_control_storage_unavailable"),
                    effects,
                )
            }
        };
        let Some(_pace) = self.pacing.acquire(&route).await else {
            return (rejected("pacing_capacity"), effects);
        };
        // The routed gate rechecks current source/credential/provider authority
        // after preparation and before the only Graph attempt.
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(error) => return (refused(rendering, error.outcome()), effects),
        };
        let sent = self
            .relay
            .whatsapp
            .send_interactive(
                credentials,
                phone_number_id,
                recipient_id,
                message_id,
                &prepared,
            )
            .await;
        drop(permit);
        ctx.gate.progress();
        let platform_id = match sent {
            Ok(id) => id,
            Err(error) => {
                let outcome = match error {
                    Error::Unconfirmed => DeliveryOutcome::Unknown {
                        reason: "native_whatsapp_acceptance_unconfirmed".into(),
                    },
                    Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
                    _ => DeliveryOutcome::Rejected {
                        reason: "native_whatsapp_send_rejected".into(),
                    },
                };
                return (refused(rendering, outcome), effects);
            }
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let receipt = Receipt {
            group: None,
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
        if receipts.put(&receipt).await.is_err() {
            return (
                refused(
                    rendering,
                    DeliveryOutcome::Unknown {
                        reason: "native_whatsapp_receipt_unconfirmed".into(),
                    },
                ),
                effects,
            );
        }
        match cards
            .bind_native_whatsapp(
                &mut prepared,
                interactions::Destination {
                    outbound_id: platform_id.clone(),
                    platform_id: platform_id.clone(),
                },
            )
            .await
        {
            Ok(reference) => (
                IntentReceipt {
                    message_parts: vec![],
                    rendering,
                    message_ref: Some(reference),
                    delivery: DeliveryOutcome::Confirmed {
                        platform_message_id: Some(platform_id),
                    },
                },
                effects,
            ),
            Err(_) => (
                refused(
                    rendering,
                    DeliveryOutcome::Unknown {
                        reason: "native_whatsapp_control_binding_unconfirmed".into(),
                    },
                ),
                effects,
            ),
        }
    }
}
