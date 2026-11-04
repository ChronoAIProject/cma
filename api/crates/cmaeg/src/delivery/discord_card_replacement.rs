//! A replacement successor gains authority only from the exact new native receipt.
use super::send::Sender;
use crate::{
    discord::{
        rest::{clear_card_confirmed, media_card_snapshots, media_edit::PreparedEdit},
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
        files: Vec<PreparedFile>,
        before: &[Snapshot],
    ) -> Result<(Replacement, PreparedEdit), RestError> {
        let replacement = Replacement::prepare(message, &files).map_err(|_| RestError::Rejected)?;
        let revision = candidate
            .card
            .revision
            .checked_add(1)
            .ok_or(RestError::Rejected)?;
        let mut content = message.clone();
        content.native_attachments.clear();
        content.clear_native_attachments = false;
        let markup = discord::render_replacement(&content, replacement.id(), revision)
            .map_err(|_| RestError::Rejected)?;
        let request = self
            .relay
            .discord
            .prepare_media_card_replacement(text, &markup, before, files)
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
        before: &[Snapshot],
    ) -> Result<(), RestError> {
        let identity = prepared.card.discord.as_mut().ok_or(RestError::Rejected)?;
        let media = identity.media_items().map_err(|_| RestError::Rejected)?;
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
        if media.is_empty() {
            return clear_card_confirmed(
                &body,
                &identity.bot_id,
                channel,
                &destination.platform_id,
                text,
                &prepared.markup,
                before.first().ok_or(RestError::Unknown)?,
            );
        }
        let after = media_card_snapshots(
            &body,
            &identity.bot_id,
            channel,
            &destination.platform_id,
            &media.iter().map(|item| item.kind).collect::<Vec<_>>(),
            text,
            &prepared.markup,
        )?;
        if after.iter().enumerate().any(|(index, snapshot)| {
            let original = before.get(index).unwrap_or(&before[0]);
            !replacement_preserves_destination(original, snapshot, before.get(index).is_some())
                || before.iter().any(|old| old.files.iter().any(|file| file.id == snapshot.files[0].id))
                || (before.get(index).is_none() && (snapshot.files[0].filename.as_ref().is_some_and(|name| name.starts_with("SPOILER_")) != matches!(snapshot.presentation, Some(Presentation::Discord { attachment_flags, .. }) if attachment_flags & 8 != 0)))
        }) {
            return Err(RestError::Unknown);
        }
        // The metadata describes the bytes sent in this PATCH, never the old upload.
        // bind() persists this confirmed proof and the successor reference in one CAS.
        identity
            .confirm_media(&after)
            .map_err(|_| RestError::Unknown)
    }
}

fn replacement_preserves_destination(
    before: &Snapshot,
    after: &Snapshot,
    preserve_spoiler: bool,
) -> bool {
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
                && (!preserve_spoiler || (attachment_flags & 8) == (old_attachment & 8))
        }
        _ => false,
    }
}
