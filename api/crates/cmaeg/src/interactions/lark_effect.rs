//! Lark deletion competes with card consumption and preserves exact accepted callbacks.
use super::{owner, Card, Error, InteractionStore, PreparedCard};
use crate::{
    channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
    tools::message_effects::lark_ledger,
};
use cmaeg_core::{ids::MessageRef, seal::MessageRefPayload};

impl InteractionStore {
    pub(crate) async fn refresh_lark_message(
        &self,
        previous: &ResolvedChannel,
        reference: &MessageRefPayload,
    ) -> Result<(), Error> {
        let caller = crate::auth::AgentCaller {
            key_hash: *previous
                .consumer_key_hash(reference.consumer.as_ref())
                .ok_or(Error::Denied)?,
        };
        let resolver = crate::channels::resolver::ChannelResolver::new(
            std::sync::Arc::new(self.channels.clone()),
            self.keys.clone(),
            Default::default(),
        );
        let current = resolver
            .resolve_current(reference.channel_id.as_str())
            .await
            .map_err(|_| Error::Unavailable)?;
        crate::tools::message_effects::lark_authority::check(reference, &current, caller)
            .map_err(|_| Error::Denied)
    }
    pub(crate) fn lark_mutations(&self) -> lark_ledger::Store {
        lark_ledger::Store::new(self.records.clone(), self.keys.clone())
    }
    pub(crate) async fn retain_lark_reference(
        &self,
        prepared: &mut PreparedCard,
        reference: &MessageRef,
    ) -> Result<(), Error> {
        prepared.card.reference = Some(reference.clone());
        prepared.version = self.save(&prepared.card, Some(prepared.version)).await?;
        Ok(())
    }
    pub(crate) async fn lark_effect_card(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
    ) -> Result<Option<(Card, u64)>, Error> {
        let Some(id) = &reference.interaction_record_id else {
            return Ok(None);
        };
        let a = reference.lark_effect.as_ref().ok_or(Error::Denied)?;
        let (card, version) = self.load(id).await?;
        if card.channel_id != channel.id
            || card.owner != channel.definition.owner.subject
            || card.event_id != reference.event_id
            || card.route_id != reference.route_id
            || card.consumer != reference.consumer
            || !owner::current(&card, channel)
            || card.chat != a.chat_id
            || card.thread != a.thread_id
            || card
                .lark
                .as_ref()
                .is_none_or(|identity| identity.platform != a.platform)
            || card.destination.as_ref().is_none_or(|destination| {
                destination.platform_id != reference.platform_message_id
                    || destination.outbound_id != a.outbound_message_id
            })
        {
            return Err(Error::Denied);
        }
        Ok(Some((card, version)))
    }
    pub(crate) async fn begin_lark_delete(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        claim: &str,
    ) -> Result<(), Error> {
        for _ in 0..2 {
            let Some((mut card, version)) = self.lark_effect_card(channel, reference).await? else {
                return Ok(());
            };
            if card.superseded {
                return Err(Error::Denied);
            }
            card.superseded = true;
            card.lark_delete = Some(claim.into());
            match self.save(&card, Some(version)).await {
                Ok(_) => return Ok(()),
                Err(Error::Denied) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Unavailable)
    }
    pub(crate) async fn finish_lark_delete(
        &self,
        reference: &MessageRefPayload,
        claim: &str,
        outcome: &DeliveryOutcome,
    ) -> Result<(), Error> {
        let Some(id) = &reference.interaction_record_id else {
            return Ok(());
        };
        if matches!(
            outcome,
            DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
        ) {
            return Ok(());
        }
        let (mut card, version) = self.load(id).await?;
        if card.lark_delete.as_deref() != Some(claim) {
            return Err(Error::Denied);
        }
        card.lark_delete = None;
        card.superseded = false;
        self.save(&card, Some(version)).await?;
        Ok(())
    }
}
pub(super) fn recovery(card: &Card, event: &str, callback: &str, action: usize) -> bool {
    card.lark_delete.is_some()
        && card.consumed.as_ref().is_some_and(|click| {
            click.event_id == event && click.callback_id == callback && click.action == action
        })
}
