//! Durable, encrypted transport interaction state. Business workflow state belongs to providers.
use crate::{
    channels::resolver::ResolvedChannel,
    providers::ProviderClient,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    activity::{Activity, InteractionResult},
    ids::{MessageRef, RecordId},
    message::OutboundMessage,
    seal::{
        open_consumer_interaction, open_interaction, seal_interaction, EventRefPayload,
        MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};
use keyboard::{actions, keyboard};
use serde_json::json;
use std::sync::Arc;

mod card;
mod keyboard;

pub use card::*;
pub use keyboard::has_controls;

const TTL: i64 = 24 * 60 * 60;
#[derive(Clone)]
pub struct InteractionStore {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    providers: Arc<ProviderClient>,
}
impl InteractionStore {
    pub fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        providers: Arc<ProviderClient>,
    ) -> Self {
        Self {
            records: records.interactions(),
            keys,
            providers,
        }
    }
    /// Every card is written as `in`: cards are the default consumer's until
    /// a later change issues consumer-owned cards, sealed as `ic`.
    async fn save(&self, card: &Card, version: Option<u64>) -> Result<u64, Error> {
        let bytes = serde_json::to_vec(card).map_err(|_| Error::Unavailable)?;
        let blob =
            seal_interaction(&self.keys, &card.id, &bytes).map_err(|_| Error::Unavailable)?;
        self.records
            .put_expiring(
                &RecordId::new(card.id.clone()).map_err(|_| Error::Denied)?,
                &card.owner,
                &blob,
                version,
                Some(card.expires_at),
            )
            .await
            .map_err(|e| match e {
                RecordError::Conflict { .. } => Error::Denied,
                _ => Error::Unavailable,
            })
    }
    pub async fn load(&self, id: &str) -> Result<(Card, u64), Error> {
        let url = self.records.record_url(id).map_err(|_| Error::Denied)?;
        let value = self.records.get(url.as_str()).await.map_err(|e| match e {
            RecordError::NotFound => Error::Denied,
            _ => Error::Unavailable,
        })?;
        let (bytes, consumer_kind) = match open_interaction(&self.keys, id, &value.blob) {
            Ok(opened) => (opened.payload, false),
            Err(_) => {
                let opened = open_consumer_interaction(&self.keys, id, &value.blob)
                    .map_err(|_| Error::Denied)?;
                (opened.payload, true)
            }
        };
        let card: Card = serde_json::from_slice(&bytes).map_err(|_| Error::Denied)?;
        // The sealed kind and the card's owner must agree: `in` is the
        // default consumer's and `ic` a named non-default consumer's, so a
        // card can never be read as belonging to another consumer.
        let owner_agrees = match &card.consumer {
            None => !consumer_kind,
            Some(consumer) => consumer_kind && !consumer.is_default(),
        };
        if !owner_agrees {
            tracing::warn!(
                card = %id,
                consumer_kind,
                "interaction card kind disagrees with its owner; refused"
            );
            return Err(Error::Denied);
        }
        if card.id != id || card.expires_at <= now() {
            return Err(Error::Denied);
        }
        Ok((card, value.version))
    }
    pub async fn prepare(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        // Cards are the default consumer's until a card names its consumer.
        let consumer = channel.consumer(None).ok_or(Error::Unavailable)?;
        let credential = consumer.credential.ok_or(Error::Unavailable)?;
        let context = self
            .providers
            .event_context(
                &consumer.provider.slug,
                &consumer.provider.binding_id,
                &event.conversation_id,
                &event.event_id,
                credential,
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        if context.channel_id != channel.id || !event.verifies_activity(&context.activity) {
            return Err(Error::Denied);
        }
        let route_id = match &event.reply {
            ReplyAddress::NyxidRelay { route_id, .. } => route_id.clone(),
            ReplyAddress::Telegram {
                bot_id, chat_id, ..
            } => crate::telegram::route(bot_id, chat_id),
            _ => return Err(Error::Denied),
        };
        let card = Card {
            id: uuid::Uuid::new_v4().to_string(),
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            event_id: event.event_id.clone(),
            route_id: route_id.clone(),
            actor: context.activity.actor.id,
            chat: context.activity.conversation.id,
            thread: context.activity.conversation.thread_id,
            expires_at: now() + TTL,
            revision: 1,
            message: message.clone(),
            destination: None,
            reference: None,
            consumed: None,
            superseded: false,
            consumer: None,
        };
        let markup = keyboard(&card)?;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
    pub async fn bind(
        &self,
        prepared: &mut PreparedCard,
        destination: Destination,
    ) -> Result<MessageRef, Error> {
        prepared.card.destination = Some(destination);
        prepared.card.reference = Some(self.reference(&prepared.card)?);
        prepared.version = self.save(&prepared.card, Some(prepared.version)).await?;
        self.reference(&prepared.card)
    }
    fn reference(&self, card: &Card) -> Result<MessageRef, Error> {
        if let Some(reference) = &card.reference {
            return Ok(reference.clone());
        }
        let destination = card.destination.as_ref().ok_or(Error::Denied)?;
        MessageRef::seal(
            &MessageRefPayload {
                channel_id: card.channel_id.clone(),
                event_id: card.event_id.clone(),
                platform_message_id: destination.platform_id.clone(),
                route_id: card.route_id.clone(),
                interaction_record_id: Some(card.id.clone()),
                consumer: None,
            },
            &self.keys,
        )
        .map_err(|_| Error::Unavailable)
    }
    pub async fn replacement(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        let (mut old, version) = self
            .load(
                reference
                    .interaction_record_id
                    .as_deref()
                    .ok_or(Error::Denied)?,
            )
            .await?;
        let destination = old.destination.as_ref().ok_or(Error::Denied)?;
        if old.channel_id != channel.id
            || old.route_id != reference.route_id
            || old.event_id != reference.event_id
            || destination.platform_id != reference.platform_message_id
            || old.superseded
            || old.consumed.is_none()
        {
            return Err(Error::Denied);
        }
        // Fence old buttons before changing an external message. Uncertain edits cannot revive them.
        old.superseded = true;
        self.save(&old, Some(version)).await?;
        let mut card = old;
        card.id = uuid::Uuid::new_v4().to_string();
        card.revision += 1;
        card.superseded = false;
        card.consumed = None;
        card.reference = None;
        card.message = message.clone();
        let markup = keyboard(&card)?;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
    pub async fn consume(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
        route: &str,
        callback_id: &str,
        data: &str,
        platform_message_id: &str,
    ) -> Result<(InteractionResult, String), Error> {
        let (id, index) = decode(data)?;
        for _ in 0..2 {
            let (mut card, version) = self.load(&id).await?;
            if card.channel_id != channel.id
                || card.route_id != route
                || card.actor != activity.actor.id
                || card.chat != activity.conversation.id
                || card.thread != activity.conversation.thread_id
                || card.superseded
                || card.reference.is_none()
                || card
                    .destination
                    .as_ref()
                    .is_none_or(|d| d.platform_id != platform_message_id)
            {
                return Err(Error::Denied);
            }
            let action = actions(&card.message)
                .get(index)
                .cloned()
                .ok_or(Error::Denied)?;
            if let Some(click) = &card.consumed {
                if click.event_id != activity.event_id.as_str()
                    || click.callback_id != callback_id
                    || click.action != index
                {
                    return Err(Error::Denied);
                }
            } else {
                card.consumed = Some(Click {
                    event_id: activity.event_id.to_string(),
                    callback_id: callback_id.into(),
                    action: index,
                    occurred_at: activity.occurred_at.clone(),
                });
                match self.save(&card, Some(version)).await {
                    Ok(_) => {}
                    Err(Error::Denied) => continue,
                    Err(e) => return Err(e),
                }
            }
            // Reference sealing is randomized. Retrying the same click must carry identical context.
            let destination = card.destination.as_ref().ok_or(Error::Denied)?;
            let values = json!({"selected":action.selected,"message_ref":card.reference,"interaction_record_id":card.id,"revision":card.revision,"platform_message_id":destination.platform_id});
            return Ok((
                InteractionResult {
                    block_id: action.block,
                    action_id: action.id,
                    values: values.as_object().unwrap().clone(),
                },
                card.consumed.as_ref().unwrap().occurred_at.clone(),
            ));
        }
        Err(Error::Denied)
    }
    pub async fn parent_reference(
        &self,
        id: &str,
        event: &EventRefPayload,
    ) -> Result<MessageRef, Error> {
        let (card, _) = self.load(id).await?;
        if card.channel_id != event.channel_id
            || card
                .consumed
                .as_ref()
                .is_none_or(|click| click.event_id != event.event_id.as_str())
        {
            return Err(Error::Denied);
        }
        self.reference(&card)
    }
}
