//! A replacement successor gains authority only from the exact new native receipt.
use super::send::Sender;
use crate::{
    discord::{
        rest::{media_card_snapshot, media_edit::PreparedEdit},
        RestError,
    },
    interactions::{
        discord,
        discord_edit::{Candidate, Replacement},
        PreparedCard,
    },
    media::outbound::PreparedFile,
    native_media::model::{Presentation, Snapshot},
};
use cmaeg_core::{channel::Secret, message::OutboundMessage};

impl Sender {
    pub(super) async fn prepare_discord_card_replacement(
        &self,
        candidate: &Candidate,
        message: &OutboundMessage,
        text: &str,
        file: PreparedFile,
        before: &Snapshot,
    ) -> Result<(Replacement, PreparedEdit), RestError> {
        let replacement = Replacement::prepare(message, &file).map_err(|_| RestError::Rejected)?;
        let revision = candidate
            .card
            .revision
            .checked_add(1)
            .ok_or(RestError::Rejected)?;
        let mut content = message.clone();
        content.native_attachments.clear();
        let markup = discord::render_replacement(&content, replacement.id(), revision)
            .map_err(|_| RestError::Rejected)?;
        let request = self
            .relay
            .discord
            .prepare_media_card_replacement(text, &markup, before, file)
            .await?;
        Ok((replacement, request))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn replace_discord_card_media(
        &self,
        prepared: &mut PreparedCard,
        credential: &Secret,
        channel: &str,
        text: &str,
        request: PreparedEdit,
        before: &Snapshot,
    ) -> Result<(), RestError> {
        let identity = prepared.card.discord.as_mut().ok_or(RestError::Rejected)?;
        let media = identity.media.as_mut().ok_or(RestError::Rejected)?;
        let destination = prepared
            .card
            .destination
            .as_ref()
            .ok_or(RestError::Rejected)?;
        let body = self
            .relay
            .discord
            .edit_media(credential, channel, &destination.platform_id, request)
            .await?;
        let after = media_card_snapshot(
            &body,
            &identity.bot_id,
            channel,
            &destination.platform_id,
            media.kind,
            text,
            &prepared.markup,
        )?;
        if !replacement_preserves_destination(before, &after) {
            return Err(RestError::Unknown);
        }
        // The metadata describes the bytes sent in this PATCH, never the old upload.
        // bind() persists this confirmed proof and the successor reference in one CAS.
        media.confirm(&after).map_err(|_| RestError::Unknown)
    }
}

fn replacement_preserves_destination(before: &Snapshot, after: &Snapshot) -> bool {
    if after.files.len() != 1 || before.files.iter().any(|old| old.id == after.files[0].id) {
        return false;
    }
    match (&before.presentation, &after.presentation) {
        (
            Some(Presentation::Discord {
                message_type: old_type,
                reply: old_reply,
                flags: old_flags,
                attachment_flags: old_attachment,
                ..
            }),
            Some(Presentation::Discord {
                message_type,
                reply,
                flags,
                attachment_flags,
                ..
            }),
        ) => {
            message_type == old_type
                && reply == old_reply
                && flags == old_flags
                && (attachment_flags & 8) == (old_attachment & 8)
        }
        _ => false,
    }
}
