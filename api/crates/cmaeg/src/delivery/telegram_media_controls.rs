//! One native upload and inline keyboard share one platform message and durable card.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
    telegram::{outcome, token},
};
use crate::{interactions::Destination, media::outbound, telegram::Error, tools::Capabilities};
use cmaeg_core::{
    message::{Block, NativeMediaKind, OutboundMessage},
    seal::{EventRefPayload, ReplyAddress},
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

impl Sender {
    pub(super) async fn telegram_media_controls_reply(
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
        if !self.relay.telegram_media_controls_enabled()
            || (event.consumer.is_some() && !self.relay.consumer_interactions_enabled())
            || message.validate().is_err()
            || message.native_attachments.len() != 1
            || message.thread_reply
            || message.reply_to.is_some()
            || message.whatsapp_template.is_some()
            || !crate::interactions::has_controls(message)
            || message.blocks.iter().any(|b| {
                !matches!(
                    b,
                    Block::Text { .. }
                        | Block::Buttons { .. }
                        | Block::Choice { .. }
                        | Block::Link { .. }
                )
            })
        {
            return (unsupported(), effects);
        }
        let Some(store) = &self.relay.interactions else {
            return (unsupported(), effects);
        };
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_telegram());
        let mut receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "attachment_preparation_failed".into(),
            },
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        let Ok(Ok(mut files)) =
            timeout_at(deadline, outbound::prepare(&message.native_attachments)).await
        else {
            return (receipt, effects);
        };
        let Some(mut file) = files.pop() else {
            return (receipt, effects);
        };
        let mut caption: Vec<_> = parts.into_iter().map(|p| p.text).collect();
        caption.extend(file.caption.take().filter(|text| !text.is_empty()));
        let caption = caption.join("\n\n");
        if caption.encode_utf16().count() > 1024 {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "interactive_media_caption_too_long".into(),
            };
            return (receipt, effects);
        }
        file.caption = (!caption.is_empty()).then_some(caption.clone());
        let kind = message.native_attachments[0].kind;
        if matches!(
            kind,
            NativeMediaKind::Sticker | NativeMediaKind::Voice | NativeMediaKind::Animation
        ) {
            return (unsupported(), effects);
        }
        let route = crate::telegram::route(bot_id, chat_id);
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
        // The card is the first side effect. No media is uploaded before its
        // complete callback authority has been durably recorded.
        if !ctx.gate.commit() {
            receipt.delivery = expired();
            return (receipt, effects);
        }
        let Ok(Ok(mut card)) = timeout_at(
            deadline,
            store.prepare_telegram_media(ctx.channel, event, message, &file),
        )
        .await
        else {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "interaction_state_unavailable".into(),
            };
            return (receipt, effects);
        };
        let Ok(Ok(upload)) = timeout_at(
            deadline,
            self.relay.telegram.prepare_media_with_markup(
                chat_id,
                thread_id.as_deref(),
                kind,
                file,
                Some(&card.markup),
            ),
        )
        .await
        else {
            return (receipt, effects);
        };
        let Ok(Some(mut slot)) = timeout_at(deadline, self.pacing.acquire(&route)).await else {
            receipt.delivery = expired();
            return (receipt, effects);
        };
        let permit = match timeout_at(deadline, ctx.gate.attempt()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(refusal)) => {
                receipt.delivery = refusal.outcome();
                return (receipt, effects);
            }
            Err(_) => {
                receipt.delivery = expired();
                return (receipt, effects);
            }
        };
        let Ok(Ok(current)) = timeout_at(deadline, store.telegram_media_channel(&card.card)).await
        else {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_destination_revoked".into(),
            };
            return (receipt, effects);
        };
        let Some(credential) = token(&current, bot_id) else {
            return (unsupported(), effects);
        };
        // One provider write, including media, caption, topic and markup.
        // Ambiguous/lost/429 responses never trigger a second send.
        let result = timeout_at(
            deadline,
            self.relay
                .telegram
                .send_media_result(credential, bot_id, upload),
        )
        .await
        .unwrap_or(Err(Error::Unconfirmed));
        drop(permit);
        ctx.gate.progress();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                if let Error::RateLimited(seconds) = error {
                    slot.cooldown(seconds);
                }
                receipt.delivery = outcome(error);
                return (receipt, effects);
            }
        };
        let media_field = match kind {
            NativeMediaKind::Image => "photo",
            NativeMediaKind::File => "document",
            NativeMediaKind::Audio => "audio",
            NativeMediaKind::Video => "video",
            NativeMediaKind::Sticker | NativeMediaKind::Voice | NativeMediaKind::Animation => {
                unreachable!()
            }
        };
        if value["from"]["id"]
            .as_i64()
            .map(|id| id.to_string())
            .as_deref()
            != Some(bot_id)
            || value["from"]["is_bot"] != true
            || value.get(media_field).is_none()
            || value["reply_markup"] != card.markup
            || value["caption"].as_str().unwrap_or("") != caption
        {
            receipt.delivery = outcome(Error::Unconfirmed);
            return (receipt, effects);
        }
        let Some(id) = value["message_id"].as_i64().map(|n| n.to_string()) else {
            receipt.delivery = outcome(Error::Unconfirmed);
            return (receipt, effects);
        };
        if self.relay.telegram_media_control_edits {
            let snapshot = crate::telegram::media_card_edit::observe(
                &value,
                bot_id,
                chat_id,
                thread_id.as_deref(),
                &id,
                kind,
                &card.markup,
            )
            .ok();
            if let Some(authority) = &mut card.card.native_telegram_media {
                authority.snapshot = snapshot;
            }
        }
        match timeout_at(
            deadline,
            store.bind_telegram_media(
                &mut card,
                Destination {
                    outbound_id: id.clone(),
                    platform_id: id.clone(),
                },
            ),
        )
        .await
        {
            Ok(Ok(reference)) => {
                receipt.message_ref = Some(reference);
                receipt.delivery = DeliveryOutcome::Confirmed {
                    platform_message_id: Some(id),
                };
            }
            _ => {
                receipt.delivery = DeliveryOutcome::Unknown {
                    reason: "interaction_receipt_unconfirmed".into(),
                };
            }
        }
        (receipt, effects)
    }
}
