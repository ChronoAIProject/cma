//! Retained native notifications enter durable admission without rerouting.
use super::*;

impl IngressState {
    pub(in crate::ingress) async fn launch_retained_message_event(
        &self,
        verified: crate::ingress::message_event::VerifiedMessageEvent,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        let Some(routing) = self.routed.as_ref().filter(|routing| {
            routing.selected_request(&verified.request)
                || (routing.native_discovery.discovery_enabled()
                    && routing
                        .native_discovery
                        .contains(verified.request.channel.definition.record_id.as_str()))
        }) else {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "message_event_durable_admission_unavailable",
            ));
        };
        let original = match routing.plans.load(&verified.original).await {
            Ok(Some(original)) => original,
            _ => {
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::ProviderUnavailable,
                    503,
                    "message_event_original_admission_unavailable",
                ))
            }
        };
        let bound = match verified.outbound_consumer.as_ref() {
            Some(consumer) => crate::ingress::message_event::bind_outbound(
                verified.request,
                &original,
                &routing.keys,
                (routing.clock)(),
                consumer,
            ),
            None => crate::ingress::message_event::bind(
                verified.request,
                &original,
                &routing.keys,
                (routing.clock)(),
            ),
        };
        let (request, plan) = match bound {
            Ok(bound) => bound,
            Err(_) => {
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::AdmissionRefused,
                    403,
                    "message_event_original_authority_unavailable",
                ))
            }
        };
        let source = VerifiedIngress {
            oversized: verified.source_bytes.len() > MAX_SOURCE_BYTES,
            body: verified.source_bytes,
            attributes_signed: true,
            received_at: Some(verified.received_at),
        };
        match routing
            .admit(&request, source, self.activity.as_ref(), false, Some(plan))
            .await
        {
            Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => outcome,
            Ok(_) => LaunchOutcome::Refused(Problem::new(
                ProblemCode::AdmissionRefused,
                422,
                "message_event_routing_required",
            )),
            Err(problem) => LaunchOutcome::Refused(problem),
        }
    }
}
