//! Native text and notifications persist exact original selection before HTTP ack.
use super::*;
use crate::channels::resolver::ResolvedChannel;
use crate::native_lark::{
    message::{self, Message, Original},
    receipts,
};
use cmaeg_core::{
    activity::{Actor, ActorKind, SourceKind},
    channel::{NativeLarkSource, Source},
    seal::ReplyAddress,
};
use std::time::Instant;
impl IngressState {
    pub(crate) async fn launch_native_lark(
        &self,
        channel: Arc<ResolvedChannel>,
        source: NativeLarkSource,
        message: Message,
        received: Instant,
        received_at: i64,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        let Some(routing) = self
            .routed
            .as_ref()
            .filter(|r| r.native_eligible(&channel.definition))
        else {
            return refused(503, "native_lark_durable_admission_unavailable");
        };
        let now = (routing.clock)();
        let identity = SourceKind::NativeLark(source.identity.clone());
        let record_identity = |event_id| crate::ingress::routing_store::EventIdentity {
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            record_id: channel.definition.record_id.clone(),
            event_id,
        };
        let mut retained = None;
        let mut outbound = None;
        let mut saved_reply = None;
        let mut author = None;
        if !message.ordinary() {
            let target = match message::original_id(&source.identity, message.message_id()) {
                Ok(id) => id,
                Err(_) => return refused(400, "native_lark_target_invalid"),
            };
            match routing.plans.load(&record_identity(target)).await {
                Ok(Some(plan)) => {
                    let first = plan.targets.first();
                    let reply = first.and_then(|target| match &target.launch.event.reply {
                        ReplyAddress::NativeLark(r) => Some(r),
                        _ => None,
                    });
                    let Some(reply) = reply.filter(|r| {
                        r.message_id == message.message_id() && r.identity == source.identity
                    }) else {
                        return refused(403, "native_lark_original_mismatch");
                    };
                    if !matches!(plan.activity.kind, ActivityKind::Message { .. })
                        || plan.source != Source::NativeLark(source.clone())
                        || plan.activity.source != identity
                    {
                        return refused(403, "native_lark_original_mismatch");
                    }
                    saved_reply = Some(reply.clone());
                    author = Some(plan.activity.actor.clone());
                    retained = Some(plan);
                }
                Ok(None) => {
                    let store = receipts::Store::new(
                        self.resolver.record_client(),
                        self.master_keys.clone(),
                    );
                    let receipt = match store
                        .load(&channel.id, &identity, message.message_id(), now)
                        .await
                    {
                        Ok(Some(r)) => r,
                        Ok(None) if received_at - message.occurred_ms() / 1000 < 120 => {
                            return refused(503, "native_lark_target_pending")
                        }
                        Ok(None) | Err(receipts::Error::Expired) => {
                            return refused(422, "native_lark_target_unavailable")
                        }
                        Err(_) => return refused(503, "native_lark_receipt_unavailable"),
                    };
                    let plan = match routing.plans.load(&receipt.original).await {
                        Ok(Some(p)) => p,
                        Ok(None) => return refused(422, "native_lark_original_unavailable"),
                        Err(_) => return refused(503, "native_lark_original_unavailable"),
                    };
                    if plan.identity.owner != channel.definition.owner.subject
                        || plan.identity.channel_id != channel.id
                        || plan.identity.record_id != channel.definition.record_id
                        || plan.source != Source::NativeLark(source.clone())
                        || plan.activity.source != identity
                        || plan.activity.conversation.id != receipt.recipient
                        || !plan.targets.iter().any(|t| {
                            t.launch.consumer == receipt.consumer
                                && t.launch.provider == receipt.provider
                                && t.launch.event.partition_key == receipt.partition_key
                                && t.launch.event.conversation_id == receipt.conversation_id
                        })
                    {
                        return refused(403, "native_lark_receipt_authority_unavailable");
                    }
                    author = Some(Actor {
                        id: source.identity.bot_open_id.clone(),
                        kind: ActorKind::Bot,
                        display_name: None,
                    });
                    saved_reply = Some(receipt.reply);
                    outbound = Some(receipt.consumer);
                    retained = Some(plan);
                }
                Err(_) => return refused(503, "native_lark_original_unavailable"),
            }
        }
        let original =
            retained
                .as_ref()
                .zip(saved_reply.as_ref())
                .zip(author)
                .map(|((p, r), a)| Original {
                    activity: &p.activity,
                    original_event: p.identity.event_id.as_str(),
                    reply: r,
                    author: a,
                });
        let request = match message::launch(
            channel.clone(),
            &source,
            &message,
            original,
            &routing.keys,
            received,
            now,
        ) {
            Ok(r) => r,
            Err(_) => return refused(403, "native_lark_admission_refused"),
        };
        let (request, plan) = if let Some(original) = &retained {
            let bound = match outbound {
                Some(c) => crate::ingress::message_event::bind_outbound(
                    request,
                    original,
                    &routing.keys,
                    now,
                    &c,
                ),
                None => crate::ingress::message_event::bind(request, original, &routing.keys, now),
            };
            match bound {
                Ok((r, p)) => (r, Some(p)),
                Err(_) => return refused(403, "native_lark_original_authority_unavailable"),
            }
        } else {
            (request, None)
        };
        // Issuance gates stop new media admissions, never duplicate retained work.
        if message.has_resources() && !(self.media_enabled && self.native_lark_media_enabled) {
            match routing
                .plans
                .load(&record_identity(request.activity.event_id.clone()))
                .await
            {
                Ok(Some(existing))
                    if serde_json::from_slice::<Message>(&existing.source_body)
                        .is_ok_and(|m| m == message) => {}
                Ok(Some(_)) => return refused(409, "native_lark_message_identity_conflict"),
                Ok(None) => return refused(422, "native_lark_media_disabled"),
                Err(_) => return refused(503, "native_lark_admission_unavailable"),
            }
        }
        // Persist bounded verified facts, never the callback verification token.
        // Ordinary keys use message_id: repeated receive envelopes do not run twice.
        let body = match serde_json::to_vec(&message) {
            Ok(b) => b,
            Err(_) => return refused(400, "native_lark_message_invalid"),
        };
        let verified = VerifiedIngress {
            oversized: body.len() > MAX_SOURCE_BYTES,
            body,
            attributes_signed: true,
            received_at: Some(received_at),
        };
        match routing
            .admit(&request, verified, self.activity.as_ref(), false, plan)
            .await
        {
            Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => outcome,
            Err(error) => LaunchOutcome::Refused(error),
            Ok(_) => refused(422, "native_lark_routing_required"),
        }
    }
}
fn refused(status: u16, reason: &'static str) -> LaunchOutcome {
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
