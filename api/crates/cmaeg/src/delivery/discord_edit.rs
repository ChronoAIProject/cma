//! Exact confirmed native Discord destinations and their current authority.
use super::{
    discord::outcome,
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::LiteralText,
    send::{refused, unsupported, SendContext, Sender},
};
use crate::{channels::resolver::ResolvedChannel, discord::rest::token, tools::Capabilities};
use cmaeg_core::{
    channel::Source,
    ids::{ConsumerId, MessageRef},
    message::{Block, OutboundMessage},
    seal::{DiscordMessageDestination, EventRefPayload, MessageRefPayload, ReplyAddress},
};
use sha2::{Digest, Sha256};

/// Only text/link blocks are editable. Native media, cards and degraded controls
/// must never accidentally grant authority over one part of a richer message.
pub(crate) fn editable_content(message: &OutboundMessage) -> bool {
    message.native_attachments.is_empty() && reference_content(message)
}

/// A reference describes the entire plain message, with or without native files.
/// Degraded cards and controls cannot become ordinary message authority.
fn reference_content(message: &OutboundMessage) -> bool {
    message.validate().is_ok()
        && message.whatsapp_template.is_none()
        && message
            .blocks
            .iter()
            .all(|block| matches!(block, Block::Text { .. } | Block::Link { .. }))
}

pub(crate) fn authority(
    channel: &ResolvedChannel,
    consumer: Option<&ConsumerId>,
    bot: &str,
) -> Option<[u8; 32]> {
    let source = channel
        .definition
        .sources
        .iter()
        .find(|source| matches!(source, Source::Discord { bot_id, .. } if bot_id == bot))?;
    let binding = channel.consumer(consumer)?;
    if binding.pending {
        return None;
    }
    let encoded = serde_json::to_vec(&serde_json::json!({
        "domain": "cmaeg.native-discord-edit.v1",
        "channel": channel.id,
        "record": channel.pointer.record_id,
        "owner": channel.definition.owner,
        "source": source,
        "consumer": consumer,
        "provider": binding.provider,
        "agent_key": channel.consumer_key_hash(consumer)?,
    }))
    .ok()?;
    Some(Sha256::digest(encoded).into())
}

impl Sender {
    pub(super) fn discord_message_ref(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message_id: &str,
        message: &OutboundMessage,
    ) -> Option<MessageRef> {
        let effects_only = !message.native_attachments.is_empty();
        let enabled = if effects_only {
            self.relay.native_discord_media_effects_enabled()
                && self.relay.native_discord_effects_enabled()
                && event.lineage.is_none()
        } else {
            self.relay.native_discord_edits_enabled()
        };
        if !enabled
            || !reference_content(message)
            || event.channel_id != channel.id
            || event.record_id != channel.pointer.record_id
        {
            return None;
        }
        let ReplyAddress::Discord {
            bot_id, channel_id, ..
        } = &event.reply
        else {
            return None;
        };
        let reference = MessageRefPayload {
            discord_message: Some(Box::new(DiscordMessageDestination {
                effects_only,
                effects: (self.relay.native_discord_effects_enabled() && event.lineage.is_none())
                    .then(|| {
                        let now = time::OffsetDateTime::now_utc().unix_timestamp();
                        cmaeg_core::seal::DiscordEffectWindow {
                            issued_at: now,
                            expires_at: now.saturating_add(86_400),
                        }
                    }),
                bot_id: bot_id.clone(),
                channel_id: channel_id.clone(),
                authority_sha256: authority(channel, event.consumer.as_ref(), bot_id)?,
            })),
            native_media_edit: None,
            received_reaction: None,
            native_lark_effect: None,
            whatsapp_reaction: None,
            lark_effect: None,
            telegram_effect: None,
            channel_id: channel.id.clone(),
            event_id: event.event_id.clone(),
            platform_message_id: message_id.into(),
            route_id: format!("discord:{bot_id}:{channel_id}"),
            interaction_record_id: None,
            relay_message: None,
            consumer: event.consumer.clone(),
        };
        MessageRef::seal(&reference, &self.keys).ok()
    }

    pub(super) async fn discord_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let Some(destination) = &reference.discord_message else {
            return unsupported();
        };
        if reference.validate().is_err()
            || destination.effects_only
            || !editable_content(message)
            || message.thread_reply
            || message.reply_to.is_some()
            || reference.channel_id != ctx.channel.id
            || authority(
                ctx.channel,
                reference.consumer.as_ref(),
                &destination.bot_id,
            ) != Some(destination.authority_sha256)
        {
            return unsupported();
        }
        let Some(credential) = token(ctx.channel, &destination.bot_id) else {
            return unsupported();
        };
        let (part, rendering) = LiteralText.whole(message, &Capabilities::native_discord());
        if part.text.is_empty() || part.text.encode_utf16().count() > 2000 {
            return refused(
                rendering,
                DeliveryOutcome::Rejected {
                    reason: "message_update_must_fit_one_part".into(),
                },
            );
        }
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return refused(rendering, refusal.outcome()),
        };
        let Some(mut slot) = self.pacing.acquire(&reference.route_id).await else {
            return refused(
                rendering,
                DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                },
            );
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return refused(rendering, refusal.outcome()),
        };
        if !ctx.gate.commit() {
            return refused(rendering, expired());
        }
        let delivery = match self
            .relay
            .discord
            .edit_text(
                credential,
                &destination.bot_id,
                &destination.channel_id,
                &reference.platform_message_id,
                &part.text,
            )
            .await
        {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(reference.platform_message_id.clone()),
            },
            Err(error) => outcome(error),
        };
        drop(permit);
        if let DeliveryOutcome::RateLimited { retry_after } = delivery {
            slot.cooldown(retry_after);
        }
        ctx.gate.progress();
        let message_ref = matches!(delivery, DeliveryOutcome::Confirmed { .. })
            .then(|| MessageRef::seal(reference, &self.keys).ok())
            .flatten();
        IntentReceipt {
            message_parts: vec![],
            message_ref,
            rendering,
            delivery,
        }
    }
}
