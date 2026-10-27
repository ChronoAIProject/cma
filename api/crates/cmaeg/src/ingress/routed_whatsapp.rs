//! Every native WhatsApp message commits before acknowledgement; rebatched retries
//! compare verified message facts then reuse the first retained signed envelope.
use super::*;
use crate::{
    ingress::{
        message_event,
        routing_store::EventIdentity,
        whatsapp::{self, VerifiedWhatsappMessage},
    },
    whatsapp::webhook::{self, Content},
};
use cmaeg_core::activity::{Actor, ActorKind};

impl IngressState {
    pub(crate) async fn launch_whatsapp_verified(
        &self,
        verified: VerifiedWhatsappMessage,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        if matches!(verified.message.content, Content::Flow { .. }) {
            return self.launch_whatsapp_flow(&verified).await;
        }
        let Some(routing) = self
            .routed
            .as_ref()
            .filter(|routing| routing.native_eligible(&verified.channel.definition))
        else {
            return refusal(503, "native_whatsapp_durable_admission_unavailable");
        };
        let now = (routing.clock)();
        let source_kind = match whatsapp::source_kind(&verified.source) {
            Some(source) => source,
            None => return refusal(400, "native_whatsapp_source_required"),
        };
        let mut retained = None;
        let mut outbound = None;
        let mut author = None;
        if let Content::Reaction { target, .. } = &verified.message.content {
            let target_event = match webhook::event_id(&verified.source, target) {
                Ok(id) => id,
                Err(_) => return refusal(400, "native_whatsapp_target_invalid"),
            };
            let identity = EventIdentity {
                owner: verified.channel.definition.owner.subject.clone(),
                channel_id: verified.channel.id.clone(),
                record_id: verified.channel.definition.record_id.clone(),
                event_id: target_event,
            };
            match routing.plans.load(&identity).await {
                Ok(Some(plan)) => {
                    let same_message = webhook::extract(&verified.source, &plan.source_body)
                        .is_ok_and(|messages| {
                            messages.iter().any(|message| {
                                message.id == *target
                                    && message.group_id.is_none()
                                    && message.sender == verified.message.sender
                                    && matches!(
                                        message.content,
                                        Content::Text { .. } | Content::Media { .. }
                                    )
                            })
                        });
                    if !same_message || !matches!(plan.activity.kind, ActivityKind::Message { .. })
                    {
                        return refusal(403, "native_whatsapp_target_authority_unavailable");
                    }
                    author = Some(plan.activity.actor.clone());
                    retained = Some(plan);
                }
                Ok(None) => {
                    let Some(store) = &self.whatsapp_receipts else {
                        return refusal(422, "native_whatsapp_target_unavailable");
                    };
                    let receipt = match store
                        .load(
                            &verified.channel.id,
                            &source_kind,
                            &verified.message.sender,
                            target,
                            now,
                        )
                        .await
                    {
                        Ok(Some(receipt)) => receipt,
                        Ok(None)
                            if verified
                                .received_at
                                .saturating_sub(verified.message.timestamp)
                                < 120 =>
                        {
                            // Meta may expose a successful send before our receipt
                            // commit settles, or deliver a reaction before another
                            // callback's original. Preserve its retry opportunity.
                            return refusal(503, "native_whatsapp_target_pending");
                        }
                        Ok(None) | Err(crate::whatsapp::receipts::Error::Expired) => {
                            return refusal(422, "native_whatsapp_target_unavailable")
                        }
                        Err(_) => return refusal(503, "native_whatsapp_receipt_unavailable"),
                    };
                    let plan = match routing.plans.load(&receipt.original).await {
                        Ok(Some(plan)) => plan,
                        Ok(None) => return refusal(422, "native_whatsapp_original_unavailable"),
                        Err(_) => return refusal(503, "native_whatsapp_original_unavailable"),
                    };
                    if !plan.targets.iter().any(|target| {
                        target.launch.consumer == receipt.consumer
                            && target.launch.provider == receipt.provider
                            && target.launch.event.partition_key == receipt.partition_key
                            && target.launch.event.conversation_id == receipt.conversation_id
                    }) || plan.identity.owner != verified.channel.definition.owner.subject
                        || plan.activity.source != source_kind
                    {
                        return refusal(403, "native_whatsapp_receipt_authority_unavailable");
                    }
                    let cmaeg_core::channel::Source::Whatsapp {
                        phone_number_id, ..
                    } = &verified.source
                    else {
                        unreachable!()
                    };
                    author = Some(Actor {
                        id: phone_number_id.clone(),
                        display_name: None,
                        kind: ActorKind::Bot,
                    });
                    outbound = Some(receipt.consumer);
                    retained = Some(plan);
                }
                Err(_) => return refusal(503, "native_whatsapp_original_unavailable"),
            }
        }
        let original = retained
            .as_ref()
            .zip(author.as_ref())
            .map(|(plan, actor)| (plan.identity.event_id.as_str(), actor));
        let mut request = match whatsapp::launch(&verified, original, &routing.keys, now) {
            Ok(request) => request,
            Err(_) => return refusal(400, "native_whatsapp_message_invalid"),
        };
        let native_control = matches!(verified.message.content, Content::Control { .. });
        if native_control {
            if let Err(outcome) = self
                .bind_whatsapp_control(&verified, &mut request, now)
                .await
            {
                return outcome;
            }
        }
        let (request, plan) = if let Some(original) = &retained {
            let bound = match &outbound {
                Some(consumer) => {
                    message_event::bind_outbound(request, original, &routing.keys, now, consumer)
                }
                None => message_event::bind(request, original, &routing.keys, now),
            };
            match bound {
                Ok((request, plan)) => (request, Some(plan)),
                Err(_) => return refusal(403, "native_whatsapp_original_authority_unavailable"),
            }
        } else {
            (request, None)
        };
        // Two attempts cover concurrent first deliveries with different batch
        // envelopes. Every attempt uses the same immutable provider message key.
        for _ in 0..2 {
            let identity = EventIdentity {
                owner: verified.channel.definition.owner.subject.clone(),
                channel_id: verified.channel.id.clone(),
                record_id: verified.channel.definition.record_id.clone(),
                event_id: request.activity.event_id.clone(),
            };
            let body = match routing.plans.load(&identity).await {
                Ok(None) => {
                    if verified.message.group_id.is_some() && !self.native_whatsapp_groups_enabled {
                        return refusal(422, "native_whatsapp_groups_disabled");
                    }
                    if matches!(verified.message.content, Content::Media { .. })
                        && !(self.media_enabled && self.native_whatsapp_media_enabled)
                    {
                        return refusal(422, "native_whatsapp_media_disabled");
                    }
                    verified.body.clone()
                }
                Ok(Some(existing)) => {
                    let same = webhook::extract(&verified.source, &existing.source_body).is_ok_and(
                        |messages| messages.iter().any(|message| message == &verified.message),
                    );
                    if !same {
                        return refusal(409, "native_whatsapp_message_identity_conflict");
                    }
                    existing.source_body
                }
                Err(_) => return refusal(503, "native_whatsapp_admission_unavailable"),
            };
            let source = VerifiedIngress {
                oversized: body.len() > MAX_SOURCE_BYTES,
                body,
                attributes_signed: true,
                received_at: Some(verified.received_at),
            };
            match routing
                .admit(
                    &request,
                    source,
                    self.activity.as_ref(),
                    native_control,
                    plan.clone(),
                )
                .await
            {
                Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => return outcome,
                Err(problem) if problem.status == 409 => continue,
                Err(problem) => return LaunchOutcome::Refused(problem),
                Ok(_) => return refusal(422, "native_whatsapp_routing_required"),
            }
        }
        refusal(409, "native_whatsapp_message_identity_conflict")
    }
}
#[path = "routed_whatsapp_control.rs"]
mod control;
fn refusal(status: u16, reason: &'static str) -> LaunchOutcome {
    LaunchOutcome::Refused(Problem::new(
        if status >= 500 {
            ProblemCode::ProviderUnavailable
        } else {
            ProblemCode::AdmissionRefused
        },
        status,
        reason,
    ))
}
