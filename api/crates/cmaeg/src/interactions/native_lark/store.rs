use super::super::{Card, InteractionStore, PreparedCard};
use super::{authority::Authority, *};
use crate::{
    channels::resolver::ResolvedChannel,
    native_lark::receipts::{Receipt, Store},
};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    seal::{EventRefPayload, MasterKeyRing, MessageRefPayload},
};

pub(super) fn valid_reference(card: &Card, keys: &MasterKeyRing) -> bool {
    let Some(authority) = &card.native_lark else {
        return false;
    };
    match (&authority.destination, &card.reference) {
        (None, None) => true,
        (Some(destination), Some(reference)) => reference.open(keys).is_ok_and(|opened| {
            let reference = opened.payload;
            is_reference(&reference)
                && reference.interaction_record_id.as_deref() == Some(card.id.as_str())
                && reference.channel_id == card.channel_id
                && reference.event_id == card.event_id
                && reference.route_id == card.route_id
                && reference.consumer == card.consumer
                && reference.platform_message_id == destination.message_id
                && reference.native_media_edit.is_none()
                && reference.received_reaction.is_none()
                && reference.native_lark_effect.is_none()
                && reference.lark_effect.is_none()
                && reference.telegram_effect.is_none()
                && reference.discord_message.is_none()
                && reference.whatsapp_reaction.is_none()
                && reference.relay_message.is_none()
        }),
        _ => false,
    }
}

impl InteractionStore {
    pub(crate) async fn prepare_native_lark(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        form: bool,
    ) -> Result<PreparedCard, Error> {
        let context = self.event_context(channel, event).await?;
        if context.conversation_id != event.conversation_id
            || !context
                .event_ref
                .open(&self.keys, super::super::now())
                .is_ok_and(|r| r.payload == *event)
        {
            return Err(Error::Denied);
        }
        let authority = Authority::prepare(channel, event, &context.activity, form)?;
        if message.validate().is_err()
            || (message.thread_reply && authority.reply().thread_id.is_none())
            || form != super::super::lark_form::requested(message)
        {
            return Err(Error::Denied);
        }
        let mut card = Card {
            native_whatsapp_flow: None,
            id: uuid::Uuid::new_v4().to_string(),
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            event_id: event.event_id.clone(),
            route_id: route(authority.reply()),
            actor: context.activity.actor.id,
            chat: context.activity.conversation.id,
            thread: context.activity.conversation.thread_id,
            expires_at: authority.issued_at + super::super::TTL,
            revision: 1,
            message: message.clone(),
            destination: None,
            reference: None,
            consumed: None,
            superseded: false,
            consumer: event.consumer.clone(),
            callback_owner: None,
            lark: None,
            lark_delete: None,
            whatsapp: None,
            native_whatsapp: None,
            discord: None,
            discord_edit: None,
            discord_form_launch: None,
            discord_effect: None,
            native_lark: Some(authority),
        };
        let custody = card
            .native_lark
            .as_ref()
            .unwrap()
            .digest(&card, channel)
            .ok_or(Error::Denied)?;
        card.native_lark.as_mut().unwrap().custody = custody;
        let markup = render(message, &card.id, card.revision, form)?.0;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
    pub(crate) async fn refresh_native_lark(
        &self,
        card: &Card,
    ) -> Result<std::sync::Arc<ResolvedChannel>, Error> {
        let channel = crate::channels::resolver::ChannelResolver::new(
            std::sync::Arc::new(self.channels.clone()),
            self.keys.clone(),
            Default::default(),
        )
        .resolve_current(card.channel_id.as_str())
        .await
        .map_err(|_| Error::Unavailable)?;
        self.native_lark_current(card, &channel).await?;
        Ok(channel)
    }
    pub(crate) async fn native_lark_current(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
    ) -> Result<(), Error> {
        let a = card.native_lark.as_ref().ok_or(Error::Denied)?;
        if !a.current(card, channel)
            || !valid_reference(card, &self.keys)
            || card.expires_at <= super::super::now()
        {
            return Err(Error::Denied);
        }
        if let Some(expected) = &a.destination {
            let actual = Store::new(self.channels.clone(), self.keys.clone())
                .load(
                    &card.channel_id,
                    &expected.source,
                    &expected.message_id,
                    super::super::now(),
                )
                .await
                .map_err(|_| Error::Unavailable)?
                .ok_or(Error::Denied)?;
            if actual != *expected {
                return Err(Error::Denied);
            }
        }
        Ok(())
    }
    pub(crate) async fn native_lark_reference(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
    ) -> Result<(Card, u64), Error> {
        if !is_reference(reference)
            || reference.native_media_edit.is_some()
            || reference.received_reaction.is_some()
            || reference.native_lark_effect.is_some()
            || reference.lark_effect.is_some()
            || reference.telegram_effect.is_some()
            || reference.discord_message.is_some()
            || reference.whatsapp_reaction.is_some()
            || reference.relay_message.is_some()
        {
            return Err(Error::Denied);
        }
        let (card, version) = self
            .load(
                reference
                    .interaction_record_id
                    .as_deref()
                    .ok_or(Error::Denied)?,
            )
            .await?;
        self.native_lark_current(&card, channel).await?;
        let a = card.native_lark.as_ref().ok_or(Error::Denied)?;
        if card.channel_id != reference.channel_id
            || card.event_id != reference.event_id
            || card.route_id != reference.route_id
            || card.consumer != reference.consumer
            || a.destination
                .as_ref()
                .is_none_or(|d| d.message_id != reference.platform_message_id)
            || card
                .reference
                .as_ref()
                .and_then(|r| r.open(&self.keys).ok())
                .is_none_or(|r| r.payload != *reference)
        {
            return Err(Error::Denied);
        }
        Ok((card, version))
    }
    pub(crate) async fn bind_native_lark(
        &self,
        prepared: &mut PreparedCard,
        receipt: Receipt,
    ) -> Result<MessageRef, Error> {
        let a = prepared.card.native_lark.as_mut().ok_or(Error::Denied)?;
        if a.destination.is_some()
            || a.replacement.as_ref().is_some_and(|p| !p.confirmed)
            || prepared.card.reference.is_some()
            || prepared.card.superseded
        {
            return Err(Error::Denied);
        }
        a.destination = Some(receipt);
        prepared.card.reference = Some(self.reference(&prepared.card)?);
        let channel = crate::channels::resolver::ChannelResolver::new(
            std::sync::Arc::new(self.channels.clone()),
            self.keys.clone(),
            Default::default(),
        )
        .resolve_current(prepared.card.channel_id.as_str())
        .await
        .map_err(|_| Error::Unavailable)?;
        self.native_lark_current(&prepared.card, &channel).await?;
        match self.save(&prepared.card, Some(prepared.version)).await {
            Ok(version) => prepared.version = version,
            Err(error) => {
                let (actual, version) = self.load(&prepared.card.id).await?;
                if version != prepared.version + 1
                    || serde_json::to_value(&actual).ok()
                        != serde_json::to_value(&prepared.card).ok()
                {
                    return Err(error);
                }
                prepared.card = actual;
                prepared.version = version;
            }
        }
        self.reference(&prepared.card)
    }
    pub(crate) async fn fence_native_lark(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
    ) -> Result<Card, Error> {
        let (mut card, version) = self.native_lark_reference(channel, reference).await?;
        if card.superseded || !super::mutation_ready(&card) {
            return Err(Error::Denied);
        }
        card.superseded = true;
        self.save(&card, Some(version)).await?;
        Ok(card)
    }
}
