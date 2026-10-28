use super::{authority, Authority, Error, Phase};
use crate::{
    channels::resolver::ResolvedChannel,
    interactions::{Card, InteractionStore},
    whatsapp::forms::{compile, hash, registry},
};
use cmaeg_core::{
    activity::Activity,
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload},
};

pub(crate) fn slot(channel: &ResolvedChannel, event: &EventRefPayload) -> String {
    registry::identifier(&(
        "native_whatsapp_form_slot_v1",
        &channel.definition.owner.subject,
        &event.channel_id,
        &event.record_id,
        &event.event_id,
        &event.consumer,
    ))
}
impl Authority {
    pub(crate) fn receipt(&self) -> Option<&crate::whatsapp::receipts::Receipt> {
        match &self.phase {
            Phase::ConfirmedUnbound { receipt, .. } | Phase::Bound { receipt, .. } => Some(receipt),
            _ => None,
        }
    }
}
impl InteractionStore {
    pub(crate) async fn prepare_whatsapp_flow(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        activity: Activity,
        key: &str,
        assets: &registry::Store,
    ) -> Result<Card, Error> {
        let compiled = compile::compile(message).map_err(|_| Error::Denied)?;
        let id = slot(channel, event);
        // A missing row alone authorizes creation. Corrupt or unavailable rows never do.
        let records = self.channels.clone().whatsapp_flows();
        let url = records.record_url(&id).map_err(|_| Error::Denied)?;
        match records.get(url.as_str()).await {
            Ok(_) => {
                return self
                    .existing_whatsapp_flow(channel, event, message, key)
                    .await
            }
            Err(crate::records::RecordError::NotFound) => {}
            Err(_) => return Err(Error::Unavailable),
        }
        // All source, event, schema and service-window checks precede capacity reservation.
        let mut authority = Authority::prepare(
            channel,
            event,
            activity,
            key,
            String::new(),
            compiled.digest.clone(),
        )?;
        let source = authority::source(channel, event).ok_or(Error::Denied)?;
        authority.asset_registry = registry::Store::asset_id(
            &registry::Store::registry_id(channel, source),
            channel.version,
            &compiled.digest,
        );
        let expires_at = authority.issued_at.saturating_add(super::super::TTL);
        let mut card = Card {
            id,
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            event_id: event.event_id.clone(),
            route_id: authority::route(event).ok_or(Error::Denied)?,
            actor: authority.activity.actor.id.clone(),
            chat: authority.activity.conversation.id.clone(),
            thread: None,
            expires_at,
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
            native_whatsapp_flow: Some(Box::new(authority)),
            native_lark: None,
            discord: None,
            discord_edit: None,
            discord_form_launch: None,
            discord_effect: None,
        };
        let digest = card
            .native_whatsapp_flow
            .as_ref()
            .unwrap()
            .digest(&card, channel)
            .ok_or(Error::Denied)?;
        card.native_whatsapp_flow.as_mut().unwrap().custody = digest;
        if !super::capacity::fits(&card) {
            return Err(Error::Denied);
        }
        let a = card.native_whatsapp_flow.as_ref().unwrap();
        let reserved = assets
            .reserve(
                channel,
                source,
                &compiled,
                &card.id,
                event.expires_at.min(a.service_until),
            )
            .await
            .map_err(|e| match e {
                crate::whatsapp::forms::Error::Unavailable => Error::Unavailable,
                _ => Error::Denied,
            })?;
        if reserved != a.asset_registry {
            return Err(Error::Denied);
        }
        if self.save_whatsapp_flow_record(&card, None).await.is_ok() {
            return Ok(card);
        }
        self.existing_whatsapp_flow(channel, event, message, key)
            .await
    }
    async fn existing_whatsapp_flow(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        key: &str,
    ) -> Result<Card, Error> {
        let (card, _) = self.load_whatsapp_flow(&slot(channel, event)).await?;
        self.whatsapp_flow_current(&card, channel).await?;
        let authority = card.native_whatsapp_flow.as_ref().ok_or(Error::Denied)?;
        if authority.original != *event
            || card.message != *message
            || authority.preparation_key != hash(&key)
        {
            return Err(Error::Denied);
        }
        Ok(card)
    }
    pub(crate) async fn whatsapp_flow_current(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
    ) -> Result<(), Error> {
        let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Denied)?;
        if card.expires_at <= super::super::now()
            || card.superseded
            || !a.current(card, channel)
            || !valid_reference(card, &self.keys)
        {
            return Err(Error::Denied);
        }
        if let Phase::Bound { receipt, .. } = &a.phase {
            let stored =
                crate::whatsapp::receipts::Store::new(self.channels.clone(), self.keys.clone())
                    .load(
                        &card.channel_id,
                        &receipt.source,
                        &receipt.recipient,
                        &receipt.message_id,
                        super::super::now(),
                    )
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .ok_or(Error::Denied)?;
            if stored != *receipt {
                return Err(Error::Denied);
            }
        }
        Ok(())
    }
    pub(crate) async fn refresh_whatsapp_flow(
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
        self.whatsapp_flow_current(card, &channel).await?;
        Ok(channel)
    }
    pub(crate) async fn save_whatsapp_flow(&self, card: &Card, version: u64) -> Result<u64, Error> {
        match self.save_whatsapp_flow_record(card, Some(version)).await {
            Ok(v) => Ok(v),
            Err(e) => {
                let (actual, v) = self.load_whatsapp_flow(&card.id).await?;
                if v == version + 1
                    && serde_json::to_value(&actual).ok() == serde_json::to_value(card).ok()
                {
                    Ok(v)
                } else {
                    Err(e)
                }
            }
        }
    }
    pub(crate) async fn bind_whatsapp_flow(
        &self,
        card: &mut Card,
        version: u64,
    ) -> Result<cmaeg_core::ids::MessageRef, Error> {
        let a = card.native_whatsapp_flow.as_mut().ok_or(Error::Denied)?;
        let Phase::ConfirmedUnbound { key, receipt } = &a.phase else {
            return Err(Error::Denied);
        };
        crate::whatsapp::receipts::Store::new(self.channels.clone(), self.keys.clone())
            .put(receipt)
            .await
            .map_err(|_| Error::Unavailable)?;
        a.phase = Phase::Bound {
            key: key.clone(),
            receipt: receipt.clone(),
        };
        card.reference = Some(self.reference(card)?);
        self.refresh_whatsapp_flow(card).await?;
        self.save_whatsapp_flow(card, version).await?;
        card.reference.clone().ok_or(Error::Denied)
    }
}
pub(super) fn valid_reference(card: &Card, keys: &cmaeg_core::seal::MasterKeyRing) -> bool {
    let Some(a) = &card.native_whatsapp_flow else {
        return true;
    };
    match (&a.phase, &card.reference) {
        (Phase::Bound { receipt, .. }, Some(reference)) => reference.open(keys).is_ok_and(|r| {
            let p: MessageRefPayload = r.payload;
            p.channel_id == card.channel_id
                && p.event_id == card.event_id
                && p.consumer == card.consumer
                && p.route_id == card.route_id
                && p.interaction_record_id.as_ref() == Some(&card.id)
                && p.platform_message_id == receipt.message_id
                && p.native_media_edit.is_none()
                && p.received_reaction.is_none()
                && p.native_lark_effect.is_none()
                && p.lark_effect.is_none()
                && p.telegram_effect.is_none()
                && p.discord_message.is_none()
                && p.whatsapp_reaction.is_none()
                && p.relay_message.is_none()
        }),
        (Phase::Bound { .. }, None) => false,
        (_, None) => true,
        _ => false,
    }
}

impl InteractionStore {
    pub(crate) async fn load_whatsapp_flow(&self, id: &str) -> Result<(Card, u64), Error> {
        let records = self.channels.clone().whatsapp_flows();
        let url = records.record_url(id).map_err(|_| Error::Denied)?;
        let row = records.get(url.as_str()).await.map_err(|e| match e {
            crate::records::RecordError::NotFound => Error::Denied,
            _ => Error::Unavailable,
        })?;
        let (opened, named) = match cmaeg_core::seal::open_interaction(&self.keys, id, &row.blob) {
            Ok(v) => (v, false),
            Err(_) => (
                cmaeg_core::seal::open_consumer_interaction(&self.keys, id, &row.blob)
                    .map_err(|_| Error::Denied)?,
                true,
            ),
        };
        let card: Card = serde_json::from_slice(&opened.payload).map_err(|_| Error::Denied)?;
        if card.id != id
            || row.version == 0
            || card.expires_at <= super::super::now()
            || card.native_whatsapp_flow.is_none()
            || !super::valid(&card, &self.keys)
            || named != card.consumer.is_some()
            || card.consumer.as_ref().is_some_and(|c| c.is_default())
        {
            return Err(Error::Denied);
        }
        Ok((card, row.version))
    }
    async fn save_whatsapp_flow_record(
        &self,
        card: &Card,
        version: Option<u64>,
    ) -> Result<u64, Error> {
        if card.native_whatsapp_flow.is_none() || !super::valid(card, &self.keys) {
            return Err(Error::Denied);
        }
        let bytes = serde_json::to_vec(card).map_err(|_| Error::Unavailable)?;
        let seal = if card.consumer.is_some() {
            cmaeg_core::seal::seal_consumer_interaction
        } else {
            cmaeg_core::seal::seal_interaction
        };
        let blob = seal(&self.keys, &card.id, &bytes).map_err(|_| Error::Unavailable)?;
        self.channels
            .clone()
            .whatsapp_flows()
            .put(
                &cmaeg_core::ids::RecordId::new(&card.id).map_err(|_| Error::Denied)?,
                &card.owner,
                &blob,
                version,
            )
            .await
            .map_err(|e| match e {
                crate::records::RecordError::Conflict { .. } => Error::Denied,
                _ => Error::Unavailable,
            })
    }
}
