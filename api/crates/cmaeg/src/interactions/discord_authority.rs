//! The saved card owns its destination, revision and original consumer authority.
use super::{discord, Card, Error, InteractionStore};
use crate::{channels::resolver::ResolvedChannel, discord::mutation};
use cmaeg_core::{activity::ActivityConversation, seal::MessageRefPayload};

pub(crate) struct Candidate {
    pub card: Card,
    pub conversation: ActivityConversation,
    pub(super) version: u64,
}
impl Candidate {
    /// Mutable callback, supersession and operation state never change receipt identity.
    pub(crate) fn effect_scope(&self) -> String {
        let c = &self.card;
        mutation::hash(&serde_json::json!({
            "id":c.id,"revision":c.revision,"owner":c.owner,"channel":c.channel_id,
            "event":c.event_id,"route":c.route_id,"actor":c.actor,"chat":c.chat,
            "thread":c.thread,"expires_at":c.expires_at,"consumer":c.consumer,
            "callback_owner":c.callback_owner,"discord":c.discord,"destination":c.destination,
            "message":c.message
        }))
    }
    pub(crate) fn mutation_destination(&self) -> String {
        let identity = self.card.discord.as_ref().expect("checked Discord card");
        let destination = self.card.destination.as_ref().expect("checked destination");
        mutation::destination(
            &identity.bot_id,
            &self.conversation.id,
            &destination.platform_id,
        )
    }
}
impl InteractionStore {
    pub(crate) async fn refreshed_discord_channel(
        &self,
        card: &Card,
        reference: &MessageRefPayload,
        admitted: &ResolvedChannel,
    ) -> Result<std::sync::Arc<ResolvedChannel>, Error> {
        let channel = crate::channels::resolver::ChannelResolver::new(
            std::sync::Arc::new(self.channels.clone()),
            self.keys.clone(),
            Default::default(),
        )
        .resolve_current(card.channel_id.as_str())
        .await
        .map_err(|_| Error::Unavailable)?;
        self.discord_card_candidate(&channel, reference).await?;
        let current = channel
            .consumer(card.consumer.as_ref())
            .ok_or(Error::Denied)?;
        let original = admitted
            .consumer(card.consumer.as_ref())
            .ok_or(Error::Denied)?;
        if current.agent_key_hash != original.agent_key_hash
            || current
                .credential
                .map(|key| crate::channels::definition::hash_secret(key.as_str()))
                != original
                    .credential
                    .map(|key| crate::channels::definition::hash_secret(key.as_str()))
        {
            return Err(Error::Denied);
        }
        Ok(channel)
    }

    pub(crate) fn mutation_store(&self) -> mutation::Store {
        mutation::Store::discord(self.records.clone(), self.keys.clone())
    }
    /// Validate current custody before both replay and new admission. Superseded
    /// cards may still replay a receipt; only admission decides whether to write.
    pub(crate) async fn discord_card_candidate(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
    ) -> Result<Candidate, Error> {
        let (card, version) = self
            .load(
                reference
                    .interaction_record_id
                    .as_deref()
                    .ok_or(Error::Denied)?,
            )
            .await?;
        let identity = card.discord.as_ref().ok_or(Error::Denied)?;
        let destination = card.destination.as_ref().ok_or(Error::Denied)?;
        let saved = card
            .reference
            .as_ref()
            .ok_or(Error::Denied)?
            .open(&self.keys)
            .map_err(|_| Error::Denied)?
            .payload;
        if reference.validate().is_err()
            || reference != &saved
            || reference.channel_id != channel.id
            || card.channel_id != channel.id
            || card.owner != channel.definition.owner.subject
            || card.route_id != reference.route_id
            || card.event_id != reference.event_id
            || card.consumer != reference.consumer
            || destination.platform_id != reference.platform_message_id
            || destination.outbound_id != destination.platform_id
            || discord::source_revision(&channel.definition)? != identity.source_revision
            || channel
                .consumer(Some(&identity.consumer))
                .is_none_or(|consumer| {
                    consumer.pending
                        || consumer.credential.is_none()
                        || consumer.provider != &identity.provider
                })
            || crate::discord::rest::token(channel, &identity.bot_id).is_none()
        {
            return Err(Error::Denied);
        }
        let conversation = ActivityConversation {
            id: card.chat.clone(),
            kind: identity.conversation_kind,
            thread_id: card.thread.clone(),
        };
        Ok(Candidate {
            card,
            conversation,
            version,
        })
    }
}
