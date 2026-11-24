//! One explicitly requested text reply starts the retained root's thread, never another destination.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    render::{LiteralText, Renderer},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    native_lark::{
        receipts::{Receipt, RETENTION},
        thread_start, Error,
    },
    tools::Capabilities,
};
use cmaeg_core::{
    activity::SourceKind,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn native_lark_thread_start(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let receipt = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.native_lark_thread_start_inner(ctx, event, message),
        )
        .await
        .unwrap_or_else(|_| {
            crate::tools::reply_targets::receipts::uncertain("native_lark_thread_start_timeout")
        });
        (receipt, SendEffects::default())
    }

    async fn native_lark_thread_start_inner(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        if !self.relay.native_lark_thread_start
            || !message.thread_reply
            || message.validate().is_err()
            || !message.blocks.is_empty()
            || !message.native_attachments.is_empty()
            || message.native_media_update.is_some()
            || message.reply_to.is_some()
            || message.whatsapp_template.is_some()
        {
            return unsupported();
        }
        let Some(text) = message
            .text
            .as_deref()
            .filter(|v| !v.trim().is_empty() && v.chars().count() <= 4000 && v.len() <= 28 * 1024)
        else {
            return unsupported();
        };
        let ReplyAddress::NativeLark(reply) = &event.reply else {
            return unsupported();
        };
        let (_, rendering) = LiteralText.render(message, &Capabilities::native_lark());
        let reject = |reason: &str| {
            refused(
                rendering.clone(),
                DeliveryOutcome::Rejected {
                    reason: reason.into(),
                },
            )
        };
        let (Some(contexts), Some(receipts)) =
            (&self.relay.interactions, &self.relay.native_lark_receipts)
        else {
            return reject("native_lark_thread_storage_unavailable");
        };
        let Ok(context) = contexts.event_context(ctx.channel, event).await else {
            return reject("native_lark_thread_context_unavailable");
        };
        if !thread_start::eligible(ctx.channel, event, &context.activity)
            || context.conversation_id != event.conversation_id
            || !context
                .event_ref
                .open(&self.keys, time::OffsetDateTime::now_utc().unix_timestamp())
                .is_ok_and(|r| r.payload == *event)
        {
            return reject("native_lark_thread_root_unavailable");
        }
        let Some(credentials) =
            crate::native_lark::credentials(&ctx.channel.definition, &event.reply)
        else {
            return reject("native_lark_destination_revoked");
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return reject("native_lark_consumer_unavailable");
        };
        let route = crate::interactions::native_lark::route(reply);
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(e) => return refused(rendering, e.outcome()),
        };
        let Some(_pace) = self.pacing.acquire(&route).await else {
            return reject("pacing_capacity");
        };
        let Ok(token) = self
            .relay
            .native_lark
            .verified_token(&reply.identity, credentials)
            .await
        else {
            return reject("native_lark_custody_unverified");
        };
        let Ok(known_thread) = self
            .relay
            .native_lark
            .thread_start_anchor(&token, reply, &context.activity)
            .await
        else {
            return reject("native_lark_thread_root_unavailable");
        };
        // Current source revision, consumer and provider are rechecked after all reads.
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(e) => return refused(rendering, e.outcome()),
        };
        if !ctx.gate.commit() {
            return refused(rendering, super::gate::expired());
        }
        let sent = self
            .relay
            .native_lark
            .start_thread_text(&token, reply, known_thread.as_deref(), text)
            .await;
        drop(permit);
        ctx.gate.progress();
        let destination = match sent {
            Ok(destination) => destination,
            Err(error) => {
                return refused(
                    rendering,
                    match error {
                        Error::Invalid | Error::Rejected => DeliveryOutcome::Rejected {
                            reason: "native_lark_thread_start_rejected".into(),
                        },
                        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
                        Error::Unconfirmed => DeliveryOutcome::Unknown {
                            reason: "native_lark_thread_start_unconfirmed".into(),
                        },
                    },
                )
            }
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
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
            message_id: destination.message_id.clone(),
            reply: destination,
            consumer: consumer.id.clone(),
            provider: consumer.provider.clone(),
            partition_key: event.partition_key.clone(),
            conversation_id: event.conversation_id.clone(),
            confirmed_at: now,
            retained_until: now + RETENTION,
        };
        if receipts.put(&receipt).await.is_err() {
            return refused(
                rendering,
                DeliveryOutcome::Unknown {
                    reason: "native_lark_thread_receipt_unconfirmed".into(),
                },
            );
        }
        IntentReceipt {
            message_parts: vec![],
            message_ref: self
                .relay
                .native_lark_effects
                .then(|| {
                    crate::tools::message_effects::native_lark_authority::confirmed(
                        ctx.channel,
                        &receipt,
                        true,
                        &self.keys,
                    )
                })
                .flatten(),
            rendering,
            delivery: DeliveryOutcome::Confirmed {
                platform_message_id: Some(receipt.message_id),
            },
        }
    }
}
