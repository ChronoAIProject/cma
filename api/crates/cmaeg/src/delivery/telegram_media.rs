//! Native media batches: prepare everything first, then authorize each platform write.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
    telegram::{outcome, token},
};
use crate::{media::outbound, telegram::Error, tools::Capabilities};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

enum NativePart<'a> {
    Text(&'a str),
    Media(crate::telegram::MediaUpload),
}

impl Sender {
    pub(super) async fn native_media_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::Telegram {
            bot_id,
            chat_id,
            thread_id,
            ..
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        if message.validate().is_err()
            || message.whatsapp_template.is_some()
            || message.thread_reply
            || message.reply_to.is_some()
            || crate::interactions::has_controls(message)
        {
            return (unsupported(), effects);
        }
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_telegram());
        let mut receipt = IntentReceipt {
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "attachment_preparation_failed".into(),
            },
        };
        let authorized = || {
            token(ctx.channel, bot_id)
                .filter(|token| crate::telegram::media_token_matches(token, bot_id))
        };
        if authorized().is_none() {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_destination_revoked".into(),
            };
            return (receipt, effects);
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        let effect_issued_at = time::OffsetDateTime::now_utc().unix_timestamp();
        let route = crate::telegram::route(bot_id, chat_id);
        let prepared = async {
            let files = outbound::prepare(&message.native_attachments).await.ok()?;
            let mut uploads = Vec::with_capacity(files.len());
            for (item, file) in message.native_attachments.iter().zip(files) {
                uploads.push(
                    self.relay
                        .telegram
                        .prepare_media(chat_id, thread_id.as_deref(), item.kind, file)
                        .await
                        .ok()?,
                );
            }
            Some(uploads)
        };
        let Ok(Some(uploads)) = timeout_at(deadline, prepared).await else {
            return (receipt, effects);
        };
        let _ticket = match timeout_at(deadline, ctx.gate.enter_route(&route)).await {
            Ok(Ok(ticket)) => ticket,
            Ok(Err(refusal)) => {
                receipt.delivery = refusal.outcome();
                return (receipt, effects);
            }
            Err(_) => {
                receipt.delivery = expired();
                return (receipt, effects);
            }
        };
        let total = parts.len() + uploads.len();
        // Text remains text; attachment captions remain per attachment. Each native
        // method is a separately authorized effect, with no automatic replay.
        let operations = parts
            .iter()
            .map(|part| NativePart::Text(part.text.as_str()))
            .chain(uploads.into_iter().map(NativePart::Media));
        let mut confirmed = 0;
        let mut first = None;
        for operation in operations {
            let Ok(Some(mut slot)) = timeout_at(deadline, self.pacing.acquire(&route)).await else {
                receipt.delivery = expired();
                break;
            };
            let permit = match timeout_at(deadline, ctx.gate.attempt()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(refusal)) => {
                    receipt.delivery = refusal.outcome();
                    break;
                }
                Err(_) => {
                    receipt.delivery = expired();
                    break;
                }
            };
            let Some(credential) = authorized() else {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "native_destination_revoked".into(),
                };
                break;
            };
            if !ctx.gate.commit() {
                receipt.delivery = expired();
                break;
            }
            let send = async {
                match operation {
                    NativePart::Text(text) => {
                        self.relay
                            .telegram
                            .send_media_text(
                                credential,
                                bot_id,
                                chat_id,
                                thread_id.as_deref(),
                                text,
                            )
                            .await
                    }
                    NativePart::Media(upload) => {
                        self.relay
                            .telegram
                            .send_media(credential, bot_id, upload)
                            .await
                    }
                }
            };
            let sent = timeout_at(deadline, send)
                .await
                .unwrap_or(Err(Error::Unconfirmed));
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(id) => {
                    confirmed += 1;
                    first.get_or_insert(id);
                    receipt.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                }
                Err(error) => {
                    if let Error::RateLimited(seconds) = error {
                        slot.cooldown(seconds);
                    }
                    receipt.delivery = outcome(error);
                    break;
                }
            }
        }
        if confirmed > 0 && confirmed < total {
            receipt.delivery = DeliveryOutcome::Unknown {
                reason: "partial_delivery".into(),
            };
        }
        // The opaque public reference names one whole native message, never the
        // first part of separate text/uploads. Captions stay on their own upload.
        if total == 1
            && confirmed == 1
            && message.native_attachments.len() == 1
            && self.relay.telegram_effects_enabled()
            && self.relay.telegram_media_effects_enabled()
        {
            if let DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            } = &receipt.delivery
            {
                if let Ok(reference) = MessageRef::seal(
                    &MessageRefPayload {
                        discord_message: None,
                        received_reaction: None,
                        native_lark_effect: None,
                        whatsapp_reaction: None,
                        lark_effect: None,
                        telegram_effect: None,
                        channel_id: event.channel_id.clone(),
                        event_id: event.event_id.clone(),
                        platform_message_id: id.clone(),
                        route_id: route,
                        interaction_record_id: None,
                        relay_message: None,
                        consumer: event.consumer.clone(),
                    },
                    &self.keys,
                ) {
                    receipt.message_ref = crate::tools::message_effects::authority::enrich(
                        &reference,
                        event,
                        ctx.channel,
                        &self.keys,
                        effect_issued_at,
                        true,
                    );
                }
            }
        }
        (receipt, effects)
    }
}
