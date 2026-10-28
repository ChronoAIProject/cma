//! Preserve the exact confirmed upload while revising its text and controls.
use super::{
    render::{RelayText, Renderer},
    send::Sender,
};
use crate::{
    discord::{rest::media_card_snapshot, RestError},
    interactions::{discord, discord_edit::Candidate, PreparedCard},
    tools::Capabilities,
};
use cmaeg_core::channel::Secret;

impl Sender {
    pub(super) async fn verify_discord_card_media(
        &self,
        candidate: &Candidate,
        credential: &Secret,
    ) -> Result<(), RestError> {
        let card = &candidate.card;
        let identity = card.discord.as_ref().ok_or(RestError::Rejected)?;
        let Some(media) = identity.media.as_ref() else {
            return Ok(());
        };
        let destination = card.destination.as_ref().ok_or(RestError::Rejected)?;
        let capabilities = Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        };
        let (parts, _) = RelayText.render(&card.message, &capabilities);
        if parts.len() != 1 {
            return Err(RestError::Rejected);
        }
        let markup = discord::render_replacement(&card.message, &card.id, card.revision)
            .map_err(|_| RestError::Rejected)?;
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.relay.discord.message(
                credential,
                &candidate.conversation.id,
                &destination.platform_id,
            ),
        )
        .await
        .map_err(|_| RestError::Unavailable)??;
        let snapshot = media_card_snapshot(
            &body,
            &identity.bot_id,
            &candidate.conversation.id,
            &destination.platform_id,
            media.kind,
            &parts[0].text,
            &markup,
        )?;
        if !media.matches_snapshot(&snapshot) {
            return Err(RestError::Rejected);
        }
        Ok(())
    }

    pub(super) async fn edit_discord_card_media(
        &self,
        prepared: &PreparedCard,
        credential: &Secret,
        channel: &str,
        text: &str,
    ) -> Result<(), RestError> {
        let card = &prepared.card;
        let identity = card.discord.as_ref().ok_or(RestError::Rejected)?;
        let media = identity.media.as_ref().ok_or(RestError::Rejected)?;
        let destination = card.destination.as_ref().ok_or(RestError::Rejected)?;
        // Omit attachments entirely: Discord keeps the existing attachment set.
        // The successful echo must prove that exact set before a successor binds.
        let body = self
            .relay
            .discord
            .edit_components_result(
                credential,
                &identity.bot_id,
                channel,
                &destination.platform_id,
                text,
                &prepared.markup,
            )
            .await?;
        let snapshot = media_card_snapshot(
            &body,
            &identity.bot_id,
            channel,
            &destination.platform_id,
            media.kind,
            text,
            &prepared.markup,
        )?;
        if !media.matches_snapshot(&snapshot) {
            return Err(RestError::Unknown);
        }
        Ok(())
    }
}
