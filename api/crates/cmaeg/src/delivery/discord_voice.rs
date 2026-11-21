//! Voice has no text/edit/control/effect enrichment, only one original destination.
use super::{
    discord::outcome,
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::discord::{rest::token, voice};
use cmaeg_core::{
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn discord_voice_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::Discord {
            bot_id,
            channel_id,
            message_id,
            thread_id,
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        if !self.relay.native_discord_voice
            || !voice::eligible(message)
            || event.lineage.is_some()
            || event.activity_sha256.is_none()
            || event.channel_id != ctx.channel.id
            || event.record_id != ctx.channel.pointer.record_id
            || thread_id
                .as_ref()
                .is_some_and(|thread| thread != channel_id)
            || (message.thread_reply && thread_id.as_ref() != Some(channel_id))
            || ctx
                .channel
                .consumer(event.consumer.as_ref())
                .is_none_or(|consumer| consumer.pending)
        {
            return (unsupported(), effects);
        }
        let result = |delivery| {
            (
                IntentReceipt {
                    message_parts: vec![],
                    message_ref: None,
                    rendering: RenderingReport {
                        outcome: RenderingOutcome::Rendered,
                        blocks: vec![],
                    },
                    delivery,
                },
                SendEffects::default(),
            )
        };
        let Some(credential) = token(ctx.channel, bot_id) else {
            return result(DeliveryOutcome::Rejected {
                reason: "native_destination_revoked".into(),
            });
        };
        let Ok(mut files) = crate::media::outbound::prepare(&message.native_attachments).await
        else {
            return result(DeliveryOutcome::Rejected {
                reason: "voice_preparation_failed".into(),
            });
        };
        let Some(prepared) = voice::prepare(
            files.remove(0),
            message.native_attachments[0].mime_type.is_none(),
            message.native_attachments[0].filename.is_none(),
            &ctx.channel.id,
            self.relay.discord_voice_decoder.as_ref(),
        )
        .await
        else {
            return result(DeliveryOutcome::Rejected {
                reason: "voice_profile_unsupported".into(),
            });
        };
        if let Err(error) = self.relay.discord.verify_bot(credential, bot_id).await {
            return result(outcome(error));
        }
        let route = format!("discord:{bot_id}:{channel_id}");
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(refusal) => return result(refusal.outcome()),
        };
        let Some(mut slot) = self.pacing.acquire(&route).await else {
            return result(DeliveryOutcome::Rejected {
                reason: "pacing_capacity".into(),
            });
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return result(refusal.outcome()),
        };
        if !ctx.gate.commit() {
            return result(expired());
        }
        let sent = self
            .relay
            .discord
            .send_voice(credential, bot_id, channel_id, message_id, &prepared)
            .await;
        drop(permit);
        ctx.gate.progress();
        result(match sent {
            Ok(id) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            },
            Err(error) => {
                if let crate::discord::rest::RestError::RateLimited(seconds) = error {
                    slot.cooldown(seconds);
                }
                outcome(error)
            }
        })
    }
}
