//! Enrich only exact single text/card receipts using their original verified activity.
use super::send::Sender;
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{channel::Source, ids::MessageRef, seal::EventRefPayload};

impl Sender {
    pub(super) async fn lark_effect_reference(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        reference: MessageRef,
        outbound: &str,
    ) -> MessageRef {
        let cmaeg_core::seal::ReplyAddress::NyxidRelay { route_id, .. } = &event.reply else {
            return reference;
        };
        if !matches!(
            channel.definition.relay_source_for_route(route_id),
            Some(Source::NyxidRelay {
                lark_message_effects: Some(_),
                ..
            })
        ) {
            return reference;
        }
        let Some(store) = &self.relay.interactions else {
            return reference;
        };
        let Ok(context) = store.event_context(channel, event).await else {
            return reference;
        };
        crate::tools::message_effects::lark_authority::enrich(
            &reference,
            event,
            &context.activity,
            channel,
            &self.keys,
            outbound,
        )
        .unwrap_or(reference)
    }
}

impl Sender {
    pub(super) async fn lark_edit_lock(
        &self,
        channel: &ResolvedChannel,
        reference: &cmaeg_core::seal::MessageRefPayload,
    ) -> Result<
        Option<(
            crate::tools::message_effects::lark_ledger::Store,
            crate::tools::message_effects::lark_ledger::Lock,
        )>,
        (),
    > {
        let Some(authority) = &reference.lark_effect else {
            return Ok(None);
        };
        let cards = self.relay.interactions.as_ref().ok_or(())?;
        cards
            .refresh_lark_message(channel, reference)
            .await
            .map_err(|_| ())?;
        let store = cards.lark_mutations();
        let held = store
            .acquire(
                crate::tools::message_effects::lark_ledger::destination(reference),
                &channel.definition.owner.subject,
                authority.expires_at,
            )
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        if cards
            .refresh_lark_message(channel, reference)
            .await
            .is_err()
        {
            let _ = store.release(held).await;
            return Err(());
        }
        Ok(Some((store, held)))
    }
}
