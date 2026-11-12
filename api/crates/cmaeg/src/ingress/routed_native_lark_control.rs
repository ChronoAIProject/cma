//! Native v2 callbacks commit an exact saved owner; consumed cards recover locally.
use super::*;
use crate::{
    channels::resolver::ResolvedChannel, interactions::Error, native_lark::callback::Callback,
    tools::message_effects::lark_ledger,
};
use cmaeg_core::ids::IdempotencyKey;
use std::time::Instant;

impl IngressState {
    pub(crate) async fn launch_native_lark_control(
        &self,
        channel: Arc<ResolvedChannel>,
        callback: Callback,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        let Some(cards) = &self.interactions else {
            return refusal(503, "native_lark_controls_storage_unavailable");
        };
        let (id, _, _) = match crate::interactions::native_lark::decode(&callback.locator) {
            Ok(parts) => parts,
            Err(_) => return refusal(403, "native_lark_control_invalid"),
        };
        let (card, _) = match cards.load(&id).await {
            Ok(card) => card,
            Err(e) => return storage_refusal(e),
        };
        if let Err(e) = cards.native_lark_current(&card, &channel).await {
            return storage_refusal(e);
        }
        let Some(destination) = card
            .native_lark
            .as_ref()
            .and_then(|a| a.destination.as_ref())
        else {
            return refusal(503, "native_lark_control_unconfirmed");
        };
        let serial =
            lark_ledger::Store::new(self.resolver.record_client(), self.master_keys.clone());
        let held = match serial
            .acquire_control(
                lark_ledger::native_destination(&destination.reply),
                &destination.original.owner,
                destination.retained_until,
                &card.id,
            )
            .await
        {
            Ok(Some(held)) => held,
            _ => return refusal(503, "native_lark_control_mutation_pending"),
        };
        let consumed = cards.consume_native_lark(&channel, &callback).await;
        if serial.release(held).await.is_err() {
            return refusal(503, "native_lark_control_fence_unconfirmed");
        }
        match consumed {
            Ok(id) => {
                self.resume_native_lark_control(&id, &channel.id.to_string())
                    .await
            }
            Err(e) => storage_refusal(e),
        }
    }
    pub(crate) async fn resume_native_lark_control(
        &self,
        id: &str,
        channel_id: &str,
    ) -> LaunchOutcome {
        let Some(cards) = &self.interactions else {
            return refusal(503, "native_lark_controls_storage_unavailable");
        };
        let channel = match self.resolver.resolve_current(channel_id).await {
            Ok(c) => c,
            Err(_) => return refusal(503, "native_lark_control_source_unavailable"),
        };
        let Some(routing) = self
            .routed
            .as_ref()
            .filter(|r| r.native_eligible(&channel.definition))
        else {
            return refusal(503, "native_lark_control_routing_unavailable");
        };
        let pending = match cards.native_lark_pending(id, &channel).await {
            Ok(p) => p,
            Err(e) => return storage_refusal(e),
        };
        // A winning card CAS blocks supersession until admission is saved. Recovery
        // therefore needs no external fence: even a crashed writer that acquired its
        // lock after consumption cannot strand the click. Clean up abandoned local
        // locks best-effort; never recover or release an external mutation lock.
        let cmaeg_core::seal::ReplyAddress::NativeLark(reply) = &pending.event.reply else {
            return refusal(403, "native_lark_control_invalid");
        };
        let serial =
            lark_ledger::Store::new(self.resolver.record_client(), self.master_keys.clone());
        if let Ok(Some(held)) = serial
            .acquire_control(
                lark_ledger::native_destination(reply),
                &channel.definition.owner.subject,
                pending.event.expires_at,
                id,
            )
            .await
        {
            let _ = serial.release(held).await;
        }
        let pending = match cards.native_lark_pending(id, &channel).await {
            Ok(p) => p,
            Err(e) => return storage_refusal(e),
        };
        let body = match serde_json::to_vec(&pending.callback) {
            Ok(b) => b,
            Err(_) => return refusal(403, "native_lark_control_invalid"),
        };
        let idempotency_key = match &pending.event.consumer {
            Some(consumer) => IdempotencyKey::for_consumer(&pending.activity.event_id, consumer),
            None => IdempotencyKey::from_event(&pending.activity.event_id),
        };
        let request = LaunchRequest {
            channel,
            activity: pending.activity,
            conversation_id: pending.event.conversation_id,
            event_ref: pending.reference,
            consumer: pending.event.consumer,
            idempotency_key,
            received_at: Instant::now(),
            trace: tracing::Span::none(),
            timing: None,
            turn_trace: None,
        };
        let source = VerifiedIngress {
            quote: crate::ingress::quoted_context::Intake::Legacy,
            oversized: body.len() > MAX_SOURCE_BYTES,
            body,
            attributes_signed: true,
            received_at: Some(pending.callback.occurred_at),
        };
        match routing
            .admit(&request, source, self.activity.as_ref(), true, None)
            .await
        {
            Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => {
                match cards.native_lark_admitted(id).await {
                    Ok(()) => outcome,
                    Err(e) => storage_refusal(e),
                }
            }
            Err(problem) => LaunchOutcome::Refused(problem),
            _ => refusal(503, "native_lark_control_admission_unavailable"),
        }
    }
}
fn storage_refusal(error: Error) -> LaunchOutcome {
    match error {
        Error::Denied => refusal(403, "native_lark_control_authority_unavailable"),
        Error::Unavailable => refusal(503, "native_lark_control_storage_unavailable"),
    }
}
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
