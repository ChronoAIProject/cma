//! One native card, one confirmed receipt, then one usable callback reference.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    interactions,
    native_lark::{
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
    pub(super) async fn native_lark_controls_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.native_lark_controls_reply_inner(ctx, event, message),
        )
        .await
        .unwrap_or_else(|_| {
            (
                crate::tools::reply_targets::receipts::uncertain("native_lark_card_send_timeout"),
                SendEffects::default(),
            )
        })
    }
    async fn native_lark_controls_reply_inner(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::NativeLark(reply) = &event.reply else {
            return (unsupported(), effects);
        };
        let form = interactions::lark_form::requested(message);
        let media = !message.native_attachments.is_empty();
        if !self.relay.native_lark_controls
            || (form && !self.relay.native_lark_forms)
            || (message.thread_reply && reply.thread_id.is_none())
            || (media
                && (!self.relay.native_lark_media_controls
                    || !self.relay.native_lark_outbound_media_enabled()))
        {
            return (unsupported(), effects);
        }
        let retained = if media {
            match interactions::native_lark::preflight_media(message) {
                Ok(retained) => retained,
                Err(_) => return (unsupported(), effects),
            }
        } else {
            message.clone()
        };
        let Ok((_, rendering)) = interactions::native_lark::render(
            &retained,
            "00000000000040008000000000000000",
            1,
            form,
        ) else {
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
        let (Some(cards), Some(receipts)) =
            (&self.relay.interactions, &self.relay.native_lark_receipts)
        else {
            return (
                rejected("native_lark_controls_storage_unavailable"),
                effects,
            );
        };
        let Some(credentials) =
            crate::native_lark::credentials(&ctx.channel.definition, &event.reply)
        else {
            return (rejected("native_lark_destination_revoked"), effects);
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return (rejected("native_lark_consumer_unavailable"), effects);
        };
        let upload = if media {
            let Ok(mut files) = crate::media::outbound::prepare(&message.native_attachments).await
            else {
                return (rejected("native_lark_media_invalid"), effects);
            };
            let Some(file) = files.pop() else {
                return (rejected("native_lark_media_invalid"), effects);
            };
            match crate::native_lark::Client::prepare_media(&message.native_attachments[0], file)
                .await
            {
                Ok(upload) => Some(upload),
                Err(_) => return (rejected("native_lark_media_invalid"), effects),
            }
        } else {
            None
        };
        let route = interactions::native_lark::route(reply);
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(t) => t,
            Err(e) => return (refused(rendering, e.outcome()), effects),
        };
        if !ctx.gate.commit() {
            return (refused(rendering, super::gate::expired()), effects);
        }
        let mut prepared = match cards
            .prepare_native_lark(ctx.channel, event, &retained, form)
            .await
        {
            Ok(p) => p,
            Err(_) => return (rejected("native_lark_control_context_unavailable"), effects),
        };
        let Some(_pace) = self.pacing.acquire(&route).await else {
            return (rejected("pacing_capacity"), effects);
        };
        let mut token = match self
            .relay
            .native_lark
            .verified_token(&reply.identity, credentials)
            .await
        {
            Ok(t) => t,
            Err(_) => return (rejected("native_lark_custody_unverified"), effects),
        };
        if cards.refresh_native_lark(&prepared.card).await.is_err() {
            return (rejected("native_lark_control_authority_revoked"), effects);
        }
        if let Some(upload) = upload {
            let permit = match ctx.gate.attempt().await {
                Ok(p) => p,
                Err(e) => return (refused(rendering, e.outcome()), effects),
            };
            let uploaded = self
                .relay
                .native_lark
                .upload_media(&token, reply, upload)
                .await;
            drop(permit);
            ctx.gate.progress();
            let uploaded = match uploaded {
                Ok(uploaded) => uploaded,
                Err(e) => return (refused(rendering, outcome(e)), effects),
            };
            if cards
                .attach_native_lark_image(&mut prepared, uploaded)
                .await
                .is_err()
            {
                return (refused(rendering, unknown()), effects);
            }
            // Upload and card delivery are separate writes. Verify current app/bot
            // custody again before using the confirmed upload in a recipient message.
            token = match self
                .relay
                .native_lark
                .verified_token(&reply.identity, credentials)
                .await
            {
                Ok(token) => token,
                Err(_) => return (rejected("native_lark_custody_unverified"), effects),
            };
        }
        // A key upload never authorizes a later card send after source/consumer revocation.
        if cards.refresh_native_lark(&prepared.card).await.is_err() {
            return (rejected("native_lark_control_authority_revoked"), effects);
        }
        let permit = match ctx.gate.attempt().await {
            Ok(p) => p,
            Err(e) => return (refused(rendering, e.outcome()), effects),
        };
        let sent = self
            .relay
            .native_lark
            .send_card(&token, reply, &prepared.markup)
            .await;
        drop(permit);
        ctx.gate.progress();
        let id = match sent {
            Ok(id) => id,
            Err(e) => return (refused(rendering, outcome(e)), effects),
        };
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
        if receipts.put(&receipt).await.is_err() {
            return (refused(rendering, unknown()), effects);
        }
        match cards.bind_native_lark(&mut prepared, receipt).await {
            Ok(reference) => (
                IntentReceipt {
                    message_ref: Some(reference),
                    rendering,
                    delivery: DeliveryOutcome::Confirmed {
                        platform_message_id: Some(id),
                    },
                },
                effects,
            ),
            Err(_) => (refused(rendering, unknown()), effects),
        }
    }
}
pub(super) fn unknown() -> DeliveryOutcome {
    DeliveryOutcome::Unknown {
        reason: "native_lark_card_mutation_unconfirmed".into(),
    }
}
pub(super) fn outcome(error: Error) -> DeliveryOutcome {
    match error {
        Error::Invalid | Error::Rejected => DeliveryOutcome::Rejected {
            reason: "native_lark_card_rejected".into(),
        },
        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
        Error::Unconfirmed => unknown(),
    }
}
