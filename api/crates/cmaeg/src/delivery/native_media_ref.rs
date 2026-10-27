//! Edit permission is issued only after the exact single-upload receipt is durable.
use super::send::{SendContext, Sender};
use crate::native_media::{
    authority,
    model::{Current, Original, Snapshot},
};
use cmaeg_core::{
    activity::Activity,
    ids::MessageRef,
    message::{Block, NativeMediaKind, OutboundMessage},
    seal::{EventRefPayload, ReplyAddress},
};

pub(super) struct MediaSeed {
    activity: Activity,
    custody: String,
    discord_channel: Option<serde_json::Value>,
    kind: NativeMediaKind,
}
impl Sender {
    pub(super) async fn prepare_media_reference(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
        whole: bool,
    ) -> Result<Option<MediaSeed>, ()> {
        let enabled = match &event.reply {
            ReplyAddress::Telegram { .. } => self.relay.telegram_media_edits,
            ReplyAddress::Discord { .. } => self.relay.discord_media_edits,
            _ => false,
        };
        if !enabled
            || !whole
            || message.native_attachments.len() != 1
            || self.relay.native_media_receipts.is_none()
            || message
                .native_attachments
                .iter()
                .any(|a| a.kind == NativeMediaKind::Sticker)
            || event.lineage.is_some()
            || message.native_attachments.iter().any(|a| {
                a.filename
                    .as_deref()
                    .is_some_and(|name| name.starts_with("SPOILER_"))
            })
            || message.whatsapp_template.is_some()
            || message.reply_to.is_some()
            || !message
                .blocks
                .iter()
                .all(|b| matches!(b, Block::Text { .. } | Block::Link { .. }))
        {
            return Ok(None);
        }
        let Some(interactions) = &self.relay.interactions else {
            return Ok(None);
        };
        let context = match interactions.event_context(ctx.channel, event).await {
            Ok(context) => context,
            Err(crate::interactions::Error::Denied) => return Err(()),
            Err(crate::interactions::Error::Unavailable) => return Ok(None),
        };
        let Some(custody) = authority::custody(ctx.channel, event, &context.activity) else {
            return Ok(None);
        };
        let destination = authority::destination(event).ok_or(())?;
        let token = authority::token(ctx.channel, &destination).ok_or(())?;
        let discord_channel = match &event.reply {
            ReplyAddress::Telegram { bot_id, .. } => {
                match self.relay.telegram.get_me(token).await {
                    Ok(identity) if identity.id == *bot_id => {}
                    Ok(_) | Err(crate::telegram::Error::Rejected) => return Err(()),
                    Err(_) => return Ok(None),
                }
                None
            }
            ReplyAddress::Discord {
                bot_id, channel_id, ..
            } => {
                match self.relay.discord.verify_media_bot(token, bot_id).await {
                    Ok(()) => {}
                    Err(crate::discord::RestError::Rejected) => return Err(()),
                    Err(_) => return Ok(None),
                }
                match self
                    .relay
                    .discord
                    .media_channel(token, channel_id, &context.activity.conversation)
                    .await
                {
                    Ok(channel) => Some(channel),
                    Err(
                        crate::discord::RestError::Rejected | crate::discord::RestError::NotFound,
                    ) => return Err(()),
                    Err(_) => return Ok(None),
                }
            }
            _ => return Err(()),
        };
        Ok(Some(MediaSeed {
            activity: context.activity,
            custody,
            discord_channel,
            kind: message.native_attachments[0].kind,
        }))
    }
    pub(super) async fn confirm_media_reference(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        seed: MediaSeed,
        snapshot: Snapshot,
        message_id: &str,
        base: Option<&MessageRef>,
    ) -> Result<MessageRef, ()> {
        if snapshot.kind != seed.kind {
            return Err(());
        }
        let confirmed_at = crate::native_media::now();
        let original = Original {
            event: event.clone(),
            activity: seed.activity,
            owner: ctx.channel.definition.owner.subject.clone(),
            discord_channel: seed.discord_channel,
            authority_sha256: seed.custody,
            message_id: message_id.into(),
            snapshot,
            confirmed_at,
            expires_at: confirmed_at.saturating_add(cmaeg_core::seal::native_media_edit::MAX_AGE),
        };
        let digest = self
            .relay
            .native_media_receipts
            .as_ref()
            .ok_or(())?
            .issue(&original)
            .await
            .map_err(|_| ())?;
        let current = Current {
            original_sha256: digest,
            revision: 0,
            snapshot: original.snapshot.clone(),
            last_operation: None,
            expires_at: original.expires_at,
            version: 0,
        };
        authority::reference(&original, &current, &self.keys, base).ok_or(())
    }
}
