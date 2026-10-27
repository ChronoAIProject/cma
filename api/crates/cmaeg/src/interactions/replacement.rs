//! Replacement retains the existing card owner and fences its prior revision.
use super::*;
use cmaeg_core::message::OutboundMessage;

impl InteractionStore {
    pub async fn replacement(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        self.replace_card(channel, reference, message, false).await
    }
    /// Replace a form using its saved native authority, even when issuance is off.
    pub async fn replacement_lark_form(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> Result<PreparedCard, Error> {
        self.replace_card(channel, reference, message, true).await
    }
    async fn replace_card(
        &self,
        channel: &ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
        native_lark_form: bool,
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
            || old
                .lark
                .as_ref()
                .and_then(|identity| identity.form.as_ref())
                .is_some_and(|form| !form.current_source(&old, channel))
            || old.native_lark.is_some()
            || old.whatsapp.is_some()
            || old.discord.is_some()
            || old.route_id != reference.route_id
            || old.event_id != reference.event_id
            || destination.platform_id != reference.platform_message_id
            || old.superseded
            || old.consumed.is_none()
            || old.consumer != reference.consumer
        {
            return Err(Error::Denied);
        }
        if native_lark_form
            && (!lark_form::requested(message)
                || old
                    .lark
                    .as_ref()
                    .is_none_or(|identity| identity.form.is_none()))
        {
            return Err(Error::Denied);
        }
        if old.lark.is_some() {
            let render = if native_lark_form {
                lark::render_form
            } else {
                lark::render
            };
            render(
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
        if let Some(identity) = &mut card.lark {
            identity.form = if native_lark_form {
                Some(
                    identity
                        .form
                        .as_ref()
                        .ok_or(Error::Denied)?
                        .replacement(message)?,
                )
            } else {
                None
            };
        }
        let markup = markup(&card)?;
        let version = self.save(&card, None).await?;
        Ok(PreparedCard {
            card,
            version,
            markup,
        })
    }
}
