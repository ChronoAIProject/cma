//! Durable, encrypted transport interaction state. Business workflow state belongs to providers.
use crate::{
    channels::resolver::ResolvedChannel,
    providers::ProviderClient,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    ids::{ConsumerId, MessageRef, RecordId},
    message::OutboundMessage,
    seal::{
        open_consumer_interaction, open_interaction, seal_consumer_interaction, seal_interaction,
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};
use keyboard::keyboard;
use std::sync::Arc;

mod card;
mod consume;
mod keyboard;
pub(crate) mod lark;
pub(crate) mod owner;
pub(crate) mod whatsapp;

pub use card::*;
pub use keyboard::has_controls;

const TTL: i64 = 24 * 60 * 60;
/// Decides whether a click may consume a card owned by the given consumer
/// (`None` is the default); see [`InteractionStore::consume`].
pub type Admit = dyn Fn(Option<&ConsumerId>) -> bool + Send + Sync;
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
    /// A default card is written as `in`, exactly as before consumers
    /// existed; a card a non-default consumer owns is sealed as `ic`, which an
    /// image that predates consumers cannot open.
    async fn save(&self, card: &Card, version: Option<u64>) -> Result<u64, Error> {
        let bytes = serde_json::to_vec(card).map_err(|_| Error::Unavailable)?;
        let seal = if card.consumer.is_some() {
            seal_consumer_interaction
        } else {
            seal_interaction
        };
        let blob = seal(&self.keys, &card.id, &bytes).map_err(|_| Error::Unavailable)?;
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
        if card.id != id
            || card.expires_at <= now()
            || !owner::valid(&card)
            || !whatsapp::valid_record(&card)
            || card.lark.as_ref().is_some_and(|identity| {
                !lark::supported(identity.platform)
                    || card.thread.is_some()
                    || card.revision == 0
                    || !matches!(
                        identity.conversation,
                        cmaeg_core::activity::ConversationKind::Private
                            | cmaeg_core::activity::ConversationKind::Group
                    )
            })
        {
            return Err(Error::Denied);
        }
        Ok((card, value.version))
    }
    /// Prepare a card for `event`, owned by the consumer the event was issued
    /// to: its context is read through that consumer's binding and
    /// credential, and its clicks are bound to that context's actor, chat and
    /// thread.
    pub async fn prepare(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        let consumer = channel
            .consumer(event.consumer.as_ref())
            .ok_or(Error::Unavailable)?;
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
        let lark = match &event.reply {
            ReplyAddress::NyxidRelay { platform, .. } if lark::supported(*platform) => {
                if context.activity.actor.kind != cmaeg_core::activity::ActorKind::Human
                    || context.activity.conversation.thread_id.is_some()
                    || context.activity.thread.is_some()
                {
                    return Err(Error::Denied);
                }
                Some(LarkIdentity {
                    platform: *platform,
                    conversation: context.activity.conversation.kind,
                })
            }
            _ => None,
        };
        let whatsapp = if matches!(
            event.reply,
            ReplyAddress::NyxidRelay {
                platform: cmaeg_core::channel::Platform::Whatsapp,
                ..
            }
        ) {
            if !whatsapp::eligible(channel, event, &context.activity) {
                return Err(Error::Denied);
            }
            let (markup, _) = whatsapp::render(message, "00000000000000000000000000000000", 1)?;
            Some(if markup["type"] == "button" {
                WhatsappKind::Button
            } else {
                WhatsappKind::List
            })
        } else {
            None
        };
        let card = Card {
            callback_owner: owner::prepare(channel, event, &context.activity)?,
            lark,
            whatsapp,
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
            // A reference never names the default, so a default card keeps
            // its exact stored bytes.
            consumer: event.consumer.clone(),
        };
        let markup = markup(&card)?;
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
                telegram_effect: None,
                channel_id: card.channel_id.clone(),
                event_id: card.event_id.clone(),
                platform_message_id: destination.platform_id.clone(),
                route_id: card.route_id.clone(),
                interaction_record_id: Some(card.id.clone()),
                relay_message: None,
                consumer: card.consumer.clone(),
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
            || !owner::current(&old, channel)
            || old.whatsapp.is_some()
            || old.route_id != reference.route_id
            || old.event_id != reference.event_id
            || destination.platform_id != reference.platform_message_id
            || old.superseded
            || old.consumed.is_none()
            || old.consumer != reference.consumer
        {
            return Err(Error::Denied);
        }
        if old.lark.is_some() {
            lark::render(
                message,
                &old.id,
                old.revision.checked_add(1).ok_or(Error::Denied)?,
            )?;
        }
        // Fence old buttons before changing an external message. Uncertain edits cannot revive them.
        old.superseded = true;
        self.save(&old, Some(version)).await?;
        // The replacement keeps the old card's consumer, and so its kind.
        let mut card = old;
        card.id = uuid::Uuid::new_v4().to_string();
        card.revision += 1;
        card.superseded = false;
        card.consumed = None;
        card.reference = None;
        card.message = message.clone();
        let markup = markup(&card)?;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
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

fn markup(card: &Card) -> Result<serde_json::Value, Error> {
    if card.whatsapp.is_some() {
        whatsapp::markup(card)
    } else if card.lark.is_some() {
        lark::markup(card)
    } else {
        keyboard(card)
    }
}
