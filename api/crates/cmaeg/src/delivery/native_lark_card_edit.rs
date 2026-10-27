//! PATCH one confirmed interactive message; a durable success precedes activation.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    native_lark_controls::{outcome, unknown},
    send::{refused, unsupported, SendContext, Sender},
};
use crate::{
    interactions,
    tools::{message_effects::lark_ledger, reply_targets::receipts},
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{MessageRefPayload, ReplyAddress},
};
impl Sender {
    pub(super) async fn native_lark_card_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            self.native_lark_card_update_inner(ctx, reference, message),
        )
        .await
        .unwrap_or_else(|_| receipts::uncertain("native_lark_card_update_timeout"))
    }
    async fn native_lark_card_update_inner(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let Some(cards) = &self.relay.interactions else {
            return unsupported();
        };
        let Ok((old, _)) = cards.native_lark_reference(ctx.channel, reference).await else {
            return unsupported();
        };
        if old.superseded {
            return unsupported();
        }
        let a = old.native_lark.as_ref().expect("checked native card");
        let Some(destination) = a.destination.clone() else {
            return unsupported();
        };
        if message.validate().is_err()
            || (message.thread_reply && destination.reply.thread_id.is_none())
            || interactions::lark_form::requested(message) != a.form
        {
            return unsupported();
        }
        let Ok((_, rendering)) = interactions::native_lark::render(
            message,
            &old.id,
            old.revision.saturating_add(1),
            a.form,
        ) else {
            return unsupported();
        };
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(t) => t,
            Err(e) => return refused(rendering, e.outcome()),
        };
        if !ctx.gate.commit() {
            return refused(rendering, super::gate::expired());
        }
        let records = cards.channel_records();
        let store = receipts::Store::new(records.clone(), self.keys.clone());
        let destination_key = lark_ledger::native_destination(&destination.reply);
        let scope = receipts::hash(&(
            reference.channel_id.clone(),
            reference.consumer.clone(),
            &destination_key,
        ));
        let claim = match store
            .claim_scoped(
                "native_lark_card_patch_v1",
                "cmaeg.native-lark-card-patch.v1",
                scope,
                &uuid::Uuid::new_v4().to_string(),
                receipts::hash(message),
                &ctx.channel.definition.owner.subject,
                destination.retained_until,
            )
            .await
        {
            Ok(receipts::Admission::Claimed(c)) => c,
            Ok(receipts::Admission::Replay(r)) => return r,
            Err(_) => return refused(rendering, unknown()),
        };
        let serial = lark_ledger::Store::new(records, self.keys.clone());
        let held = match serial
            .acquire(
                destination_key,
                &ctx.channel.definition.owner.subject,
                destination.retained_until,
            )
            .await
        {
            Ok(Some(h)) => h,
            _ => {
                let r = refused(rendering, unknown());
                let _ = store.finish(claim, &r).await;
                return r;
            }
        };
        let result = self
            .native_lark_patch_locked(ctx, reference, message, cards, &destination)
            .await;
        if store.finish(claim, &result).await.is_err() {
            return refused(rendering, unknown());
        }
        if !matches!(result.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        result
    }
    async fn native_lark_patch_locked(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
        cards: &interactions::InteractionStore,
        destination: &crate::native_lark::receipts::Receipt,
    ) -> IntentReceipt {
        // Superseding the old revision precedes every external write, including an
        // ambiguous PATCH. No failure path restores old clickable authority.
        let mut prepared = match cards
            .replace_native_lark(ctx.channel, reference, message)
            .await
        {
            Ok(p) => p,
            Err(_) => return unsupported(),
        };
        let rendering = interactions::native_lark::render(
            message,
            &prepared.card.id,
            prepared.card.revision,
            prepared.card.native_lark.as_ref().unwrap().form,
        )
        .unwrap()
        .1;
        let rejected = || {
            refused(
                rendering.clone(),
                DeliveryOutcome::Rejected {
                    reason: "native_lark_card_authority_unavailable".into(),
                },
            )
        };
        let Some(_pace) = self.pacing.acquire(&reference.route_id).await else {
            return rejected();
        };
        let channel = match cards.refresh_native_lark(&prepared.card).await {
            Ok(c) => c,
            Err(_) => return rejected(),
        };
        let reply = ReplyAddress::NativeLark(destination.reply.clone());
        let Some(credentials) = crate::native_lark::credentials(&channel.definition, &reply) else {
            return rejected();
        };
        let token = match self
            .relay
            .native_lark
            .verified_token(&destination.reply.identity, credentials)
            .await
        {
            Ok(t) => t,
            Err(_) => return rejected(),
        };
        if self
            .relay
            .native_lark
            .card_message(&token, &destination.reply)
            .await
            .is_err()
        {
            return rejected();
        }
        // Fresh local custody after all native metadata/token reads.
        if cards.refresh_native_lark(&prepared.card).await.is_err() {
            return rejected();
        }
        let permit = match ctx.gate.attempt().await {
            Ok(p) => p,
            Err(e) => return refused(rendering, e.outcome()),
        };
        if let Err(error) = self
            .relay
            .native_lark
            .patch_card(&token, &destination.reply, &prepared.markup)
            .await
        {
            return refused(rendering, outcome(error));
        }
        drop(permit);
        ctx.gate.progress();
        if cards
            .native_lark_patch_confirmed(&mut prepared)
            .await
            .is_err()
        {
            return refused(rendering, unknown());
        }
        match cards
            .bind_native_lark(&mut prepared, destination.clone())
            .await
        {
            Ok(reference) => IntentReceipt {
                message_ref: Some(reference),
                rendering,
                delivery: DeliveryOutcome::Confirmed {
                    platform_message_id: Some(destination.message_id.clone()),
                },
            },
            Err(_) => refused(rendering, unknown()),
        }
    }
}
