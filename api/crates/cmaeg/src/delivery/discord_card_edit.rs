//! Retained native card references edit one exact message through a fenced chain.
use super::{
    discord::outcome,
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::{LiteralText, Renderer},
    send::{refused, unsupported, SendContext, Sender},
};
use crate::{
    discord::rest::token,
    interactions::{
        discord,
        discord_edit::{self, Admission},
    },
    tools::Capabilities,
};
use cmaeg_core::{message::OutboundMessage, seal::MessageRefPayload};

impl Sender {
    pub(super) async fn discord_card_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let Some(store) = &self.relay.interactions else {
            return unsupported();
        };
        let candidate = match store
            .discord_edit_candidate(ctx.channel, reference, message)
            .await
        {
            Ok(Admission::Replay(receipt)) => return receipt,
            Ok(Admission::Ready(candidate)) => candidate,
            Err(_) => return unsupported(),
        };
        let replacement =
            !message.native_attachments.is_empty() || message.clear_native_attachments;
        let mut content = message.clone();
        content.native_attachments.clear();
        content.clear_native_attachments = false;
        let form = crate::interactions::discord_form::schema(message).is_ok();
        let Some(identity) = candidate.card.discord.as_ref() else {
            return unsupported();
        };
        if ((message.native_attachments.len() > 1 || message.clear_native_attachments)
            && !self.relay.discord_media_card_sets)
            || (replacement && !self.relay.discord_media_card_replacements_enabled())
            || (candidate.card.revision == 1
                && (!self.relay.discord_card_edits_enabled()
                    || (reference.consumer.is_some()
                        && !self.relay.consumer_interactions_enabled())))
            || (candidate.card.revision == 1
                && identity.media_items().is_ok_and(|items| !items.is_empty())
                && !self.relay.discord_media_card_edits_enabled())
            || (discord::multiselect_requested(message)
                && !self.relay.discord_multiselect_enabled()
                && !discord::multiselect_requested(&candidate.card.message))
            || (message.thread_reply && candidate.conversation.thread_id.is_none())
            || (form && identity.form.is_none() && !self.relay.discord_forms_enabled())
            || (!form
                && discord::render_replacement(
                    &content,
                    "00000000000000000000000000000000",
                    candidate.card.revision.saturating_add(1),
                )
                .is_err())
        {
            return unsupported();
        }
        let capabilities = Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        };
        let (parts, mut rendering) = LiteralText.render(&content, &capabilities);
        if form {
            crate::interactions::discord_form::report(message, &mut rendering);
        }
        if parts.len() != 1 || (parts[0].text.is_empty() && !discord::requested(message)) {
            return unsupported();
        }
        let Some(credential) = token(ctx.channel, &identity.bot_id) else {
            return unsupported();
        };
        if let Err(error) = self
            .relay
            .discord
            .verify_bot(credential, &identity.bot_id)
            .await
        {
            return refused(rendering, outcome(error));
        }
        match self
            .relay
            .discord
            .verify_component_channel(
                credential,
                &candidate.conversation.id,
                &candidate.conversation,
            )
            .await
        {
            Ok(thread) if thread == identity.thread => {}
            Ok(_) => return unsupported(),
            Err(error) => return refused(rendering, outcome(error)),
        }
        let files = if replacement {
            match crate::discord::outbound_media::prepare(&message.native_attachments).await {
                Ok(files) => Some(files),
                _ => {
                    return refused(
                        rendering,
                        DeliveryOutcome::Rejected {
                            reason: "attachment_preparation_failed".into(),
                        },
                    )
                }
            }
        } else {
            None
        };
        let bot = identity.bot_id.clone();
        let native_channel = candidate.conversation.id.clone();
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(error) => return refused(rendering, error.outcome()),
        };
        let Some(mut slot) = self.pacing.acquire(&reference.route_id).await else {
            return unsupported();
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(error) => return refused(rendering, error.outcome()),
        };
        if !ctx.gate.commit() {
            return refused(rendering, expired());
        }
        // Complete download and multipart serialization while the original callback
        // remains usable. Discord uploads files inside PATCH, not via a separate API.
        let prepared_before = if replacement {
            match self.verify_discord_card_media(&candidate, credential).await {
                Ok(snapshot) => snapshot,
                Err(error) => return refused(rendering, outcome(error)),
            }
        } else {
            None
        };
        let replacement_request =
            if let (Some(files), Some(before)) = (files, prepared_before.as_ref()) {
                match self
                    .prepare_discord_card_replacement(
                        &candidate,
                        message,
                        &parts[0].text,
                        files,
                        before,
                    )
                    .await
                {
                    Ok(prepared) => Some(prepared),
                    Err(error) => {
                        return refused(rendering, outcome(error));
                    }
                }
            } else {
                None
            };
        if let Err(error) = self.relay.discord.verify_media_bot(credential, &bot).await {
            return refused(rendering, outcome(error));
        }
        let current_channel = match store
            .refreshed_discord_channel(&candidate.card, reference, ctx.channel)
            .await
        {
            Ok(channel) => channel,
            Err(_) => return unsupported(),
        };
        let serial = store.mutation_store();
        let held = match serial
            .acquire(
                candidate.mutation_destination(),
                &ctx.channel.definition.owner.subject,
                candidate.card.expires_at,
            )
            .await
        {
            Ok(Some(held)) => held,
            Ok(None) => {
                return refused(
                    rendering,
                    DeliveryOutcome::Rejected {
                        reason: "native_message_effect_busy".into(),
                    },
                )
            }
            Err(_) => return discord_edit::uncertain(),
        };
        // Refresh after the shared destination lock, before the card CAS. A
        // callback may have won in the meantime; retain its consumed result.
        let candidate = match store
            .discord_edit_candidate(&current_channel, reference, message)
            .await
        {
            Ok(Admission::Ready(candidate)) => candidate,
            Ok(Admission::Replay(receipt)) => {
                let _ = serial.release(held).await;
                return receipt;
            }
            Err(_) => {
                let _ = serial.release(held).await;
                return discord_edit::uncertain();
            }
        };
        // The platform may have changed independently. Refuse before card CAS
        // if its current upload/content/controls no longer match the retained proof.
        let before = match self.verify_discord_card_media(&candidate, credential).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = serial.release(held).await;
                return refused(rendering, outcome(error));
            }
        };
        let current_channel = match store
            .refreshed_discord_channel(&candidate.card, reference, ctx.channel)
            .await
        {
            Ok(channel) => channel,
            Err(_) => {
                let _ = serial.release(held).await;
                return unsupported();
            }
        };
        let candidate = match store
            .discord_edit_candidate(&current_channel, reference, message)
            .await
        {
            Ok(Admission::Ready(candidate)) => candidate,
            Ok(Admission::Replay(receipt)) => {
                let _ = serial.release(held).await;
                return receipt;
            }
            Err(_) => {
                let _ = serial.release(held).await;
                return discord_edit::uncertain();
            }
        };
        let original_card = candidate.card.clone();
        let (claimed, request) = if let Some((replacement, request)) = replacement_request {
            (
                store
                    .begin_discord_media_replacement(*candidate, message, replacement)
                    .await,
                Some(request),
            )
        } else {
            (store.begin_discord_edit(*candidate, message).await, None)
        };
        let mut claim = match claimed {
            Ok(claim) => claim,
            Err(_) => return discord_edit::uncertain(),
        };
        let edited = if store
            .refreshed_discord_channel(&original_card, reference, ctx.channel)
            .await
            .is_err()
        {
            Err(crate::discord::RestError::Rejected)
        } else if let (Some(request), Some(before)) = (request, before) {
            self.replace_discord_card_media(
                &mut claim.prepared,
                credential,
                &native_channel,
                &parts[0].text,
                request,
                &before,
            )
            .await
        } else if claim
            .prepared
            .card
            .discord
            .as_ref()
            .is_some_and(|i| i.media_items().is_ok_and(|items| !items.is_empty()))
        {
            self.edit_discord_card_media(
                &claim.prepared,
                credential,
                &native_channel,
                &parts[0].text,
            )
            .await
        } else {
            self.relay
                .discord
                .edit_components(
                    credential,
                    &bot,
                    &native_channel,
                    &reference.platform_message_id,
                    &parts[0].text,
                    &claim.prepared.markup,
                )
                .await
        };
        let delivery = match edited {
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
        let mut receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery,
        };
        if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
            let destination = claim
                .prepared
                .card
                .destination
                .clone()
                .expect("checked card destination");
            match store.bind(&mut claim.prepared, destination).await {
                Ok(reference) => receipt.message_ref = Some(reference),
                Err(_) => {
                    receipt.delivery = DeliveryOutcome::Unknown {
                        reason: "discord_card_edit_receipt_unconfirmed".into(),
                    }
                }
            }
        }
        if store.finish_discord_edit(claim, &receipt).await.is_err() {
            return discord_edit::uncertain();
        }
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. })
            && serial.release(held).await.is_err()
        {
            tracing::warn!("native card mutation fence remains held after completed edit");
        }
        receipt
    }
}
