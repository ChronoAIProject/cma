//! Preserve the exact confirmed upload while revising its text and controls.
use super::{
    render::{LiteralText, Renderer},
    send::Sender,
};
use crate::{
    discord::{rest::media_card_snapshots, RestError},
    interactions::{discord, discord_edit::Candidate, PreparedCard},
    tools::Capabilities,
};
use cmaeg_core::channel::Secret;

impl Sender {
    pub(super) async fn verify_discord_card_media(
        &self,
        candidate: &Candidate,
        credential: &Secret,
    ) -> Result<Option<Vec<crate::native_media::model::Snapshot>>, RestError> {
        let card = &candidate.card;
        let identity = card.discord.as_ref().ok_or(RestError::Rejected)?;
        let media = identity.media_items().map_err(|_| RestError::Rejected)?;
        if media.is_empty() {
            return Ok(None);
        }
        let destination = card.destination.as_ref().ok_or(RestError::Rejected)?;
        let capabilities = Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        };
        let (parts, _) = LiteralText.render(&card.message, &capabilities);
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
        let snapshot = media_card_snapshots(
            &body,
            &identity.bot_id,
            &candidate.conversation.id,
            &destination.platform_id,
            &media.iter().map(|item| item.kind).collect::<Vec<_>>(),
            &parts[0].text,
            &markup,
        )?;
        if media.len() != snapshot.len()
            || !media
                .iter()
                .zip(&snapshot)
                .all(|(item, snapshot)| item.matches_snapshot(snapshot))
        {
            return Err(RestError::Rejected);
        }
        Ok(Some(snapshot))
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
        let media = identity.media_items().map_err(|_| RestError::Rejected)?;
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
        let snapshot = media_card_snapshots(
            &body,
            &identity.bot_id,
            channel,
            &destination.platform_id,
            &media.iter().map(|item| item.kind).collect::<Vec<_>>(),
            text,
            &prepared.markup,
        )?;
        if media.len() != snapshot.len()
            || !media
                .iter()
                .zip(&snapshot)
                .all(|(item, snapshot)| item.matches_snapshot(snapshot))
        {
            return Err(RestError::Unknown);
        }
        Ok(())
    }
}
