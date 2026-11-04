//! Sanitized static Flow completions retain one exact consumer and a local outbox.
use super::*;
use crate::{interactions::Error, whatsapp::forms::callback::Callback};
use cmaeg_core::ids::IdempotencyKey;
impl IngressState {
    pub(crate) async fn launch_whatsapp_flow(
        &self,
        verified: &crate::ingress::whatsapp::VerifiedWhatsappMessage,
    ) -> LaunchOutcome {
        let Some(cards) = &self.interactions else {
            return refusal(503, "whatsapp_flow_storage_unavailable");
        };
        let callback = match Callback::from_message(&verified.source, &verified.message) {
            Ok(c) => c,
            Err(_) => return refusal(403, "whatsapp_flow_invalid"),
        };
        match cards
            .consume_whatsapp_flow(&verified.channel, &callback)
            .await
        {
            Ok(id) => {
                self.resume_whatsapp_flow(&id, &verified.channel.id.to_string())
                    .await
            }
            Err(e) => storage(e),
        }
    }
    pub(crate) async fn resume_whatsapp_flow(&self, id: &str, channel_id: &str) -> LaunchOutcome {
        let Some(cards) = &self.interactions else {
            return refusal(503, "whatsapp_flow_storage_unavailable");
        };
        let channel = match self.resolver.resolve_current(channel_id).await {
            Ok(c) => c,
            Err(_) => return refusal(503, "whatsapp_flow_source_unavailable"),
        };
        let Some(routing) = self
            .routed
            .as_ref()
            .filter(|r| r.native_eligible(&channel.definition))
        else {
            return refusal(503, "whatsapp_flow_routing_unavailable");
        };
        let pending = match cards.whatsapp_flow_pending(id, &channel).await {
            Ok(p) => p,
            Err(e) => return storage(e),
        };
        let body = match serde_json::to_vec(&pending.callback) {
            Ok(b) => b,
            Err(_) => return refusal(403, "whatsapp_flow_invalid"),
        };
        let idempotency_key = match &pending.event.consumer {
            Some(c) => IdempotencyKey::for_consumer(&pending.activity.event_id, c),
            None => IdempotencyKey::from_event(&pending.activity.event_id),
        };
        let request = LaunchRequest {
            channel,
            activity: pending.activity,
            conversation_id: pending.event.conversation_id,
            event_ref: pending.reference,
            consumer: pending.event.consumer,
            idempotency_key,
            received_at: std::time::Instant::now(),
            trace: tracing::Span::none(),
            timing: None,
        };
        let source = VerifiedIngress {
            quote: crate::ingress::quoted_context::Intake::Legacy,
            oversized: body.len() > MAX_SOURCE_BYTES,
            body,
            attributes_signed: true,
            received_at: Some(pending.callback.timestamp),
        };
        match routing
            .admit(&request, source, self.activity.as_ref(), true, None)
            .await
        {
            Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => {
                match cards.whatsapp_flow_admitted(id).await {
                    Ok(()) => outcome,
                    Err(e) => storage(e),
                }
            }
            Err(p) => LaunchOutcome::Refused(p),
            _ => refusal(503, "whatsapp_flow_admission_unavailable"),
        }
    }
}
fn storage(error: Error) -> LaunchOutcome {
    match error {
        Error::Denied => refusal(403, "whatsapp_flow_authority_unavailable"),
        Error::Unavailable => refusal(503, "whatsapp_flow_storage_unavailable"),
    }
}
fn refusal(status: u16, reason: &str) -> LaunchOutcome {
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
