//! Media-card creation and binding use the existing durable card CAS protocol.
use super::{telegram_media, Card, Destination, Error, InteractionStore, PreparedCard};
use crate::{channels::resolver::ResolvedChannel, media::outbound::PreparedFile};
use cmaeg_core::{ids::MessageRef, message::OutboundMessage, seal::EventRefPayload};
use std::sync::Arc;

impl InteractionStore {
    pub(crate) async fn prepare_telegram_media(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        file: &PreparedFile,
    ) -> Result<PreparedCard, Error> {
        if message.native_attachments.len() != 1 || !super::has_controls(message) {
            return Err(Error::Denied);
        }
        let context = self.event_context(channel, event).await?;
        if context.conversation_id != event.conversation_id {
            return Err(Error::Denied);
        }
        let authority = telegram_media::Authority::prepare(
            channel,
            event,
            &context.activity,
            message.native_attachments[0].kind,
            file,
        )?;
        let mut retained = message.clone();
        // Interaction records are limited to 64 KiB. Retain compact custody,
        // never the source URL or potentially megabytes of base64 upload data.
        retained.native_attachments.clear();
        let mut prepared = self
            .prepare_with_context(channel, event, &retained, context)
            .await?;
        prepared.card.native_telegram_media = Some(authority);
        prepared.card.expires_at = prepared.card.expires_at.min(event.expires_at);
        if !telegram_media::valid(&prepared.card) || prepared.card.expires_at <= super::now() {
            return Err(Error::Denied);
        }
        // An interrupted preparation leaves an unbound card, which cannot be
        // consumed. The marker is durable before the only platform write.
        prepared.version = self.save(&prepared.card, Some(prepared.version)).await?;
        Ok(prepared)
    }

    /// Re-read current source/provider custody and credentials immediately before send.
    pub(crate) async fn telegram_media_channel(
        &self,
        card: &Card,
    ) -> Result<Arc<ResolvedChannel>, Error> {
        let channel = crate::channels::resolver::ChannelResolver::new(
            Arc::new(self.channels.clone()),
            self.keys.clone(),
            Default::default(),
        )
        .resolve_current(card.channel_id.as_str())
        .await
        .map_err(|_| Error::Unavailable)?;
        if card.expires_at <= super::now()
            || !telegram_media::valid(card)
            || !card
                .native_telegram_media
                .as_ref()
                .is_some_and(|a| a.current(card, &channel))
        {
            return Err(Error::Denied);
        }
        Ok(channel)
    }

    pub(crate) async fn bind_telegram_media(
        &self,
        prepared: &mut PreparedCard,
        destination: Destination,
    ) -> Result<MessageRef, Error> {
        if prepared.card.native_telegram_media.is_none()
            || prepared.card.reference.is_some()
            || prepared.card.destination.is_some()
            || destination.outbound_id != destination.platform_id
            || !destination
                .platform_id
                .parse::<i64>()
                .is_ok_and(|n| n > 0 && n.to_string() == destination.platform_id)
        {
            return Err(Error::Denied);
        }
        self.telegram_media_channel(&prepared.card).await?;
        match self.bind(prepared, destination).await {
            Ok(reference) => Ok(reference),
            Err(error) => {
                // A lost database response is recoverable only by reading the
                // exact committed card/version. Never resend the platform write.
                let (stored, version) = self.load(&prepared.card.id).await?;
                if version != prepared.version.saturating_add(1)
                    || serde_json::to_value(&stored).map_err(|_| Error::Unavailable)?
                        != serde_json::to_value(&prepared.card).map_err(|_| Error::Unavailable)?
                {
                    return Err(error);
                }
                prepared.card = stored;
                prepared.version = version;
                self.reference(&prepared.card)
            }
        }
    }
}
