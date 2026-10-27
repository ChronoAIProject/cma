//! Durable, encrypted transport interaction state. Business workflow state belongs to providers.
use crate::{
    channels::resolver::ResolvedChannel,
    providers::ProviderClient,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    ids::{ConsumerId, MessageRef, RecordId},
    seal::{
        open_consumer_interaction, open_interaction, seal_consumer_interaction, seal_interaction,
        EventRefPayload, MasterKeyRing, MessageRefPayload,
    },
};
use keyboard::keyboard;
use std::sync::Arc;

mod card;
mod consume;
pub(crate) mod discord;
pub(crate) mod discord_authority;
pub(crate) mod discord_consume;
pub(crate) mod discord_edit;
pub(crate) mod discord_effect;
pub(crate) mod discord_form;
pub(crate) mod discord_form_store;
#[cfg(test)]
mod discord_form_tests;
pub(crate) mod discord_form_values;
mod discord_receipts;
#[cfg(test)]
mod discord_tests;
mod keyboard;
pub(crate) mod lark;
mod lark_effect;
pub(crate) mod lark_form;
mod lark_form_authority;
pub(crate) mod owner;
mod prepare;
mod replacement;
pub(crate) mod whatsapp;
pub(crate) mod whatsapp_authority;
mod whatsapp_binding;

pub use card::*;
pub use keyboard::has_controls;

const TTL: i64 = 24 * 60 * 60;
/// Decides whether a click may consume a card owned by the given consumer
/// (`None` is the default); see [`InteractionStore::consume`].
pub type Admit = dyn Fn(Option<&ConsumerId>) -> bool + Send + Sync;
#[derive(Clone)]
pub struct InteractionStore {
    channels: RecordClient,
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
            channels: records.clone(),
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
            || !discord::valid(&card)
            || !discord_edit::valid(&card)
            || !discord_effect::valid(&card)
            || !discord_form::valid(&card)
            || !lark_form_authority::valid(&card)
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
                discord_message: None,
                received_reaction: None,
                whatsapp_reaction: None,
                lark_effect: None,
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
    if card.discord.is_some() {
        if card
            .discord
            .as_ref()
            .is_some_and(|identity| identity.form.is_some())
        {
            discord_form::button(card)
        } else {
            discord::render(&card.message, &card.id, card.revision)
        }
    } else if card.whatsapp.is_some() {
        whatsapp::markup(card)
    } else if card.lark.is_some() {
        lark::markup(card)
    } else {
        keyboard(card)
    }
}

#[cfg(test)]
pub(crate) mod whatsapp_native_fixture;
#[cfg(test)]
mod whatsapp_native_tests;
