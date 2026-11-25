//! Native text, media and approved templates retain the bound phone and exact send targets.
use super::{
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    ingress::routing_store::EventIdentity,
    tools::Capabilities,
    whatsapp::{
        outbound::{MediaUpload, UploadedMedia},
        receipts::{Receipt, RETENTION},
        structured::PreparedStructured,
        templates::PreparedTemplate,
        Error,
    },
};
use cmaeg_core::{
    activity::SourceKind,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

use std::{collections::VecDeque, time::Duration};
use tokio::time::{timeout_at, Instant};

enum NativePart {
    Text(String),
    Upload(MediaUpload),
    Media(UploadedMedia),
    Template(PreparedTemplate),
    Structured(PreparedStructured),
}

impl Sender {
    pub(super) async fn whatsapp_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        // Native sends do not consume or change a relay acknowledgement token.
        let effects = SendEffects::default();
        if crate::whatsapp::forms::compile::requested(message) {
            return (rejected("whatsapp_form_preparation_required"), effects);
        }
        let (app_id, waba_id, phone_number_id, recipient_id, message_id, group) = match &event.reply
        {
            ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            } => (
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
                None,
            ),
            ReplyAddress::WhatsappGroup(reply) => (
                &reply.app_id,
                &reply.waba_id,
                &reply.phone_number_id,
                &reply.group_id,
                &reply.message_id,
                Some(reply),
            ),
            _ => return (unsupported(), effects),
        };
        if group.is_some()
            && (!self.relay.native_whatsapp_groups_enabled()
                || crate::interactions::has_controls(message)
                || message
                    .native_attachments
                    .iter()
                    .any(|item| item.kind == cmaeg_core::message::NativeMediaKind::Sticker))
        {
            return (unsupported(), effects);
        }
        if group.is_none()
            && self.relay.native_whatsapp_interactions_enabled()
            && crate::interactions::whatsapp::requested(message)
        {
            return self.whatsapp_controls_reply(ctx, event, message).await;
        }
        let structured = match message.structured_content.as_ref() {
            Some(content) if group.is_none() && self.relay.native_whatsapp_structured_replies => {
                match PreparedStructured::new(phone_number_id, recipient_id, message_id, content) {
                    Ok(prepared) => Some(prepared),
                    Err(_) => {
                        return (
                            rejected("native_whatsapp_structured_content_invalid"),
                            effects,
                        )
                    }
                }
            }
            Some(_) => return (unsupported(), effects),
            None => None,
        };
        let media = !message.native_attachments.is_empty();
        if (media
            && (!self.relay.native_whatsapp_outbound_media_enabled()
                || crate::interactions::has_controls(message)))
            || (message.whatsapp_template.is_some()
                && !self.relay.native_whatsapp_templates_enabled())
            || message.reply_to.is_some()
            || message.thread_reply
            || message.validate().is_err()
        {
            return (unsupported(), effects);
        }
        let template = match message.whatsapp_template.as_ref() {
            Some(template) => {
                match group.map_or_else(
                    || PreparedTemplate::new(phone_number_id, recipient_id, message_id, template),
                    |reply| PreparedTemplate::for_group(reply, template),
                ) {
                    Ok(template) => Some(template),
                    Err(_) => return (rejected("native_whatsapp_template_invalid"), effects),
                }
            }
            None => None,
        };
        let Some(credentials) = crate::whatsapp::credentials(&ctx.channel.definition, &event.reply)
            .filter(|credentials| credentials.validate().is_ok())
        else {
            return (rejected("native_whatsapp_destination_revoked"), effects);
        };
        let Some(store) = &self.relay.whatsapp_receipts else {
            return (
                rejected("native_whatsapp_receipt_storage_unavailable"),
                effects,
            );
        };
        let Some(consumer) = ctx.channel.consumer(event.consumer.as_ref()) else {
            return (rejected("native_whatsapp_consumer_unavailable"), effects);
        };
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_whatsapp());
        let mut result = IntentReceipt {
            message_parts: vec![],
            rendering,
            ..rejected("empty_message")
        };
        let deadline = ctx
            .gate
            .transport_deadline()
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
        if structured.is_some() {
            let Some(interactions) = &self.relay.interactions else {
                return (unsupported(), effects);
            };
            if !matches!(
                timeout_at(
                    deadline,
                    crate::whatsapp::structured::authorized(
                        interactions.channel_records(),
                        self.keys.clone(),
                        ctx.channel,
                        event,
                    )
                )
                .await,
                Ok(true)
            ) {
                return (
                    rejected("native_whatsapp_original_event_unavailable"),
                    effects,
                );
            }
        }
        let prepared = async {
            if let Some(structured) = structured {
                return Some(VecDeque::from([NativePart::Structured(structured)]));
            }
            if let Some(template) = template {
                return Some(VecDeque::from([NativePart::Template(template)]));
            }
            let files = crate::media::outbound::prepare(&message.native_attachments)
                .await
                .ok()?;
            let mut operations: VecDeque<_> = parts
                .into_iter()
                .map(|part| NativePart::Text(part.text))
                .collect();
            for (item, file) in message.native_attachments.iter().zip(files) {
                operations.push_back(NativePart::Upload(
                    crate::whatsapp::Client::prepare_media(item, file)
                        .await
                        .ok()?,
                ));
            }
            Some(operations)
        };
        let Ok(Some(mut operations)) = timeout_at(deadline, prepared).await else {
            result.delivery = DeliveryOutcome::Rejected {
                reason: "native_whatsapp_media_invalid".into(),
            };
            return (result, effects);
        };
        let route = format!("whatsapp:{app_id}:{waba_id}:{phone_number_id}:{recipient_id}");
        let _ticket = match timeout_at(deadline, ctx.gate.enter_route(&route)).await {
            Ok(Ok(ticket)) => ticket,
            Ok(Err(refusal)) => {
                result.delivery = refusal.outcome();
                return (result, effects);
            }
            Err(_) => {
                result.delivery = super::gate::expired();
                return (result, effects);
            }
        };
        let mut confirmed = 0;
        let mut first = None;
        while let Some(operation) = operations.pop_front() {
            let Ok(Some(_pace)) = timeout_at(deadline, self.pacing.acquire(&route)).await else {
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            if let Some(reply) = group {
                if !matches!(
                    timeout_at(
                        deadline,
                        self.relay.whatsapp.verify_groups(
                            credentials,
                            phone_number_id,
                            std::slice::from_ref(&reply.group_id),
                            Some(&reply.participant_id)
                        )
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    result.delivery = DeliveryOutcome::Rejected {
                        reason: "native_whatsapp_group_custody_unavailable".into(),
                    };
                    break;
                }
            }
            let permit = match timeout_at(deadline, ctx.gate.attempt()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(refusal)) => {
                    result.delivery = refusal.outcome();
                    break;
                }
                Err(_) => {
                    result.delivery = super::gate::expired();
                    break;
                }
            };
            if !ctx.gate.commit() {
                result.delivery = super::gate::expired();
                break;
            }
            // The routed gate rechecks source/credential revision before each HTTP
            // write, including the gap between upload and recipient send.
            let upload = matches!(&operation, NativePart::Upload(_));
            let send = async {
                match operation {
                    NativePart::Structured(structured) => self
                        .relay
                        .whatsapp
                        .send_structured(credentials, structured)
                        .await
                        .map(Some),
                    NativePart::Template(template) => self
                        .relay
                        .whatsapp
                        .send_template(credentials, template)
                        .await
                        .map(Some),
                    NativePart::Text(text) if group.is_some() => self
                        .relay
                        .whatsapp
                        .send_group_text(credentials, group.unwrap(), &text)
                        .await
                        .map(Some),
                    NativePart::Text(text) => self
                        .relay
                        .whatsapp
                        .send_text(
                            credentials,
                            phone_number_id,
                            recipient_id,
                            message_id,
                            &text,
                        )
                        .await
                        .map(Some),
                    NativePart::Upload(upload) => {
                        let uploaded = self
                            .relay
                            .whatsapp
                            .upload_media(credentials, phone_number_id, upload)
                            .await?;
                        operations.push_front(NativePart::Media(uploaded));
                        Ok(None)
                    }
                    NativePart::Media(uploaded) if group.is_some() => self
                        .relay
                        .whatsapp
                        .send_group_media(credentials, group.unwrap(), uploaded)
                        .await
                        .map(Some),
                    NativePart::Media(uploaded) => self
                        .relay
                        .whatsapp
                        .send_media(
                            credentials,
                            phone_number_id,
                            recipient_id,
                            message_id,
                            uploaded,
                        )
                        .await
                        .map(Some),
                }
            };
            let sent = super::trace::native::part(
                confirmed,
                upload,
                async {
                    timeout_at(deadline, send)
                        .await
                        .unwrap_or(Err(Error::Unconfirmed))
                },
                super::trace::native::whatsapp,
            )
            .await;
            drop(permit);
            ctx.gate.progress();
            match sent {
                Ok(None) => continue,
                Ok(Some(platform_id)) => {
                    let now = time::OffsetDateTime::now_utc().unix_timestamp();
                    let receipt = Receipt {
                        group: group.cloned(),
                        kind: if group.is_some() {
                            "native_whatsapp_group_receipt_v1"
                        } else {
                            "native_whatsapp_receipt_v1"
                        }
                        .into(),
                        original: EventIdentity {
                            owner: ctx.channel.definition.owner.subject.clone(),
                            channel_id: event.channel_id.clone(),
                            record_id: event.record_id.clone(),
                            event_id: event.event_id.clone(),
                        },
                        source: SourceKind::Whatsapp {
                            app_id: app_id.clone(),
                            waba_id: waba_id.clone(),
                            phone_number_id: phone_number_id.clone(),
                        },
                        recipient: recipient_id.clone(),
                        message_id: platform_id.clone(),
                        consumer: consumer.id.clone(),
                        provider: consumer.provider.clone(),
                        partition_key: event.partition_key.clone(),
                        conversation_id: event.conversation_id.clone(),
                        confirmed_at: now,
                        retained_until: now + RETENTION,
                    };
                    confirmed += 1;
                    if !super::gate::within_deadline(
                        ctx.gate,
                        async { store.put(&receipt).await.is_ok() },
                        false,
                    )
                    .await
                    {
                        result.delivery = DeliveryOutcome::Unknown {
                            reason: "native_whatsapp_receipt_unconfirmed".into(),
                        };
                        break;
                    }
                    if first.is_none() {
                        first = Some(platform_id);
                    }
                    result.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                }
                Err(error) => {
                    result.delivery = match error {
                        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
                        Error::Rejected | Error::Invalid | Error::CustodyUnproved => {
                            DeliveryOutcome::Rejected {
                                reason: "native_whatsapp_send_rejected".into(),
                            }
                        }
                        Error::Unconfirmed => DeliveryOutcome::Unknown {
                            reason: "native_whatsapp_acceptance_unconfirmed".into(),
                        },
                    };
                    break;
                }
            }
        }
        if confirmed > 0
            && !matches!(
                result.delivery,
                DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
            )
        {
            result.delivery = DeliveryOutcome::Unknown {
                reason: "native_whatsapp_partial_delivery".into(),
            };
        }
        (result, effects)
    }
}
fn rejected(reason: &'static str) -> IntentReceipt {
    let mut result = unsupported();
    result.delivery = DeliveryOutcome::Rejected {
        reason: reason.into(),
    };
    result
}
