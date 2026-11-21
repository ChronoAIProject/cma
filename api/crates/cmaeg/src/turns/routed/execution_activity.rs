use super::Journal;
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    providers::{RecoveryEvent, ResponsesEvent},
    turns::execution_receipt::{verified_change, ExecutionScope, ExecutionVerifiers},
};
use std::sync::Arc;

impl Journal {
    pub(crate) fn with_execution_verifiers(mut self, verifiers: Arc<ExecutionVerifiers>) -> Self {
        self.execution_verifiers = verifiers;
        self
    }

    /// Called after the authenticated frame, its effect and the resulting
    /// durable checkpoint have all been validated. Observation is optional
    /// and can neither initiate delivery nor change its result.
    pub(super) async fn observe_execution(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        frame: &RecoveryEvent,
    ) {
        if self.activity.is_none() || !matches!(frame.event, ResponsesEvent::Completed { .. }) {
            return;
        }
        let provider = target.launch.provider.slug.as_str();
        let Some(verifier) = self.execution_verifiers.verifier(provider) else {
            return;
        };
        let scope = ExecutionScope {
            provider: provider.into(),
            channel_record_id: plan.identity.record_id.to_string(),
            event_id: plan.identity.event_id.to_string(),
            consumer_id: target.launch.consumer.to_string(),
            response_id: frame.response_id.clone(),
        };
        if let Some(change) = verified_change(verifier, &scope, &frame.event) {
            self.observe(&plan.identity, change).await;
        }
    }
}
