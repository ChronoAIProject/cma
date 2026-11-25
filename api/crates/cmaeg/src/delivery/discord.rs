//! Native Discord reply delivery, independent of relay envelopes and credentials.
use super::{
    gate::{expired, within_deadline},
    intents::{ConfirmedMessagePart, DeliveryOutcome, IntentReceipt},
    render::{LiteralText, RenderedPart, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    discord::{
        outbound_media,
        rest::{token, RestError},
    },
    tools::Capabilities,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn discord_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        if crate::discord::voice::requested(message) {
            return self.discord_voice_reply(ctx, event, message).await;
        }
        if self.relay.discord_media_controls_enabled()
            && !message.native_attachments.is_empty()
            && message
                .blocks
                .iter()
                .any(|block| matches!(block, cmaeg_core::message::Block::Form { .. }))
        {
            return (unsupported(), SendEffects::default());
        }
        if self.relay.discord_interactions_enabled() {
            if crate::interactions::discord::requested(message) {
                return self.discord_controls(ctx, event, message, None).await;
            }
            if self.relay.discord_forms_enabled()
                && super::discord_controls::form_eligible(message)
                && event.lineage.is_none()
                && event.activity_sha256.is_some()
                && (event.consumer.is_none() || self.relay.consumer_interactions_enabled())
            {
                let Some(store) = &self.relay.interactions else {
                    return (unsupported(), SendEffects::default());
                };
                let Ok(context) = store.event_context(ctx.channel, event).await else {
                    return (unsupported(), SendEffects::default());
                };
                if crate::interactions::discord::eligible(ctx.channel, event, &context.activity) {
                    return self
                        .discord_controls(ctx, event, message, Some(context))
                        .await;
                }
                // Verified but ineligible context retains its established text
                // rendering. No card, platform write or fresh authority exists.
            }
        }
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
        if message.validate().is_err()
            || message.whatsapp_template.is_some()
            || (message.thread_reply && thread_id.as_ref() != Some(channel_id))
            || message.reply_to.is_some()
        {
            return (unsupported(), effects);
        }
        let (mut parts, rendering) = LiteralText.render(message, &Capabilities::native_discord());
        let mut receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "empty_message".into(),
            },
        };
        let Some(credential) = token(ctx.channel, bot_id) else {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_destination_revoked".into(),
            };
            return (receipt, effects);
        };
        if let Err(error) = within_deadline(
            ctx.gate,
            self.relay.discord.verify_bot(credential, bot_id),
            Err(RestError::Unknown),
        )
        .await
        {
            receipt.delivery = outcome(error);
            return (receipt, effects);
        }
        let target_receipt = match within_deadline(
            ctx.gate,
            self.discord_reaction_receipt(ctx.channel, event),
            Err(crate::discord::GatewayError::Unavailable),
        )
        .await
        {
            Ok(receipt) => receipt,
            Err(_) => {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "reaction_target_custody_unavailable".into(),
                };
                return (receipt, effects);
            }
        };
        let files = match outbound_media::prepare(&message.native_attachments).await {
            Ok(files) => files,
            Err(_) => {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "attachment_preparation_failed".into(),
                };
                return (receipt, effects);
            }
        };
        if parts.is_empty() && !files.is_empty() {
            parts.push(RenderedPart {
                text: String::new(),
            });
        }
        let seed = match self
            .prepare_media_reference(ctx, event, message, parts.len() == 1)
            .await
        {
            Ok(seed) => seed,
            Err(()) => {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "native_media_custody_unavailable".into(),
                };
                return (receipt, effects);
            }
        };
        let mut media_result = None;
        let route = format!("discord:{bot_id}:{channel_id}");
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(refusal) => {
                receipt.delivery = refusal.outcome();
                return (receipt, effects);
            }
        };
        let split_refs = self.relay.native_split_text_refs
            && parts.len() > 1
            && super::discord_edit::editable_content(message);
        let mut confirmed = 0;
        let mut first = None;
        for (index, part) in parts.iter().enumerate() {
            let Some(mut slot) = within_deadline(ctx.gate, self.pacing.acquire(&route), None).await
            else {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let permit = match ctx.gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => {
                    receipt.delivery = refusal.outcome();
                    break;
                }
            };
            if index == 0 && !ctx.gate.commit() {
                receipt.delivery = expired();
                break;
            }
            let send = async {
                if seed.is_some() {
                    match self
                        .relay
                        .discord
                        .send_components_result(
                            credential,
                            bot_id,
                            channel_id,
                            (index == 0).then_some(message_id.as_str()),
                            &part.text,
                            &files,
                            None,
                        )
                        .await
                    {
                        Ok(body) if body["author"]["bot"] == true => {
                            let id = body["id"]
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(RestError::Unknown);
                            media_result = Some(body);
                            id
                        }
                        Ok(_) => Err(RestError::Unknown),
                        Err(error) => Err(error),
                    }
                } else {
                    self.relay
                        .discord
                        .send(
                            credential,
                            bot_id,
                            channel_id,
                            (index == 0).then_some(message_id.as_str()),
                            &part.text,
                            if index == 0 { &files } else { &[] },
                        )
                        .await
                }
            };
            let sent = super::trace::native::part(
                index,
                false,
                within_deadline(ctx.gate, send, Err(RestError::Unknown)),
                super::trace::native::discord,
            )
            .await;
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(id) => {
                    confirmed += 1;
                    if let Some(target) = &target_receipt {
                        if within_deadline(
                            ctx.gate,
                            target.confirm(&id),
                            Err(crate::discord::GatewayError::Unavailable),
                        )
                        .await
                        .is_err()
                        {
                            receipt.delivery = DeliveryOutcome::Unknown {
                                reason: "reaction_target_receipt_unconfirmed".into(),
                            };
                            break;
                        }
                    }
                    if split_refs {
                        if let Some(message_ref) =
                            self.discord_message_ref(ctx.channel, event, &id, message)
                        {
                            receipt.message_parts.push(ConfirmedMessagePart {
                                index: index as u16,
                                message_ref,
                            });
                        }
                    }
                    if first.is_none() {
                        first = Some(id);
                    }
                    receipt.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                }
                Err(error) => {
                    if let RestError::RateLimited(seconds) = error {
                        slot.cooldown(seconds);
                    }
                    receipt.delivery = outcome(error);
                    break;
                }
            }
        }
        if confirmed > 0 && confirmed < parts.len() {
            receipt.delivery = DeliveryOutcome::Unknown {
                reason: "partial_delivery".into(),
            };
        }
        if confirmed == 1 && parts.len() == 1 {
            if let DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            } = &receipt.delivery
            {
                receipt.message_ref = self.discord_message_ref(ctx.channel, event, id, message);
            }
        }
        if let (
            Some(seed),
            Some(body),
            DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            },
        ) = (seed, media_result, &receipt.delivery)
        {
            let parsed = crate::discord::rest::media_edit::snapshot(
                &body,
                bot_id,
                channel_id,
                id,
                message.native_attachments[0].kind,
            );
            let confirmed = match parsed {
                Ok(snapshot)
                    if snapshot.caption == files[0].caption.as_deref().unwrap_or("")
                        && snapshot.content == parts[0].text
                        && snapshot.files[0].filename.as_deref()
                            == Some(files[0].filename.as_str())
                        && snapshot.files[0].size == Some(files[0].bytes.len() as u64)
                        && snapshot.files[0]
                            .mime
                            .as_deref()
                            .is_none_or(|v| v == files[0].mime) =>
                {
                    self.confirm_media_reference(
                        ctx,
                        event,
                        seed,
                        snapshot,
                        id,
                        receipt.message_ref.as_ref(),
                    )
                    .await
                }
                _ => Err(()),
            };
            // Edit enrichment is optional: a confirmed ordinary upload keeps its
            // existing effects reference when this narrower receipt cannot be issued.
            if let Ok(reference) = confirmed {
                receipt.message_ref = Some(reference);
            }
        }
        (receipt, effects)
    }
}
pub(super) fn outcome(error: RestError) -> DeliveryOutcome {
    match error {
        RestError::RateLimited(retry_after) => DeliveryOutcome::RateLimited { retry_after },
        RestError::Rejected | RestError::NotFound => DeliveryOutcome::Rejected {
            reason: "native_request_rejected".into(),
        },
        RestError::Unavailable => DeliveryOutcome::Rejected {
            reason: "native_dispatch_unavailable".into(),
        },
        RestError::Unknown => DeliveryOutcome::Unknown {
            reason: "native_delivery_unconfirmed".into(),
        },
    }
}
