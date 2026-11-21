use super::{ExecutionError, Journal, Lease};
use crate::{
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
    providers::{RecoveryEvent, ResponsesEvent},
    turns::execution_receipt::{verified_change, ExecutionScope, ExecutionVerifiers},
};
use std::{sync::Arc, time::Duration};

impl Journal {
    pub(crate) fn with_execution_verifiers(mut self, verifiers: Arc<ExecutionVerifiers>) -> Self {
        self.execution_verifiers = verifiers;
        self
    }

    /// Retain only bounded verified evidence, before acknowledging the frame.
    /// Delivery has already settled. Failure here cannot authorize a resend.
    pub(super) async fn acknowledge_frame(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
        frame: &RecoveryEvent,
    ) -> Result<(), ExecutionError> {
        self.retain_execution(plan, target, lease, frame).await?;
        self.acknowledge_delivery(lease, &frame.response_id, frame.sequence_number)
            .await?;
        let _ = self
            .project_execution(&plan.identity, target.launch.consumer.as_str(), lease)
            .await;
        Ok(())
    }

    async fn retain_execution(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
        frame: &RecoveryEvent,
    ) -> Result<(), ExecutionError> {
        if self.activity.is_none() || !matches!(frame.event, ResponsesEvent::Completed { .. }) {
            return Ok(());
        }
        let checkpoint = lease
            .state
            .delivery
            .as_ref()
            .ok_or(ExecutionError::Corrupt)?;
        if checkpoint.execution_attribution.is_some() {
            return Ok(());
        }
        if !checkpoint.terminal
            || !checkpoint.status.settled()
            || checkpoint.cursor != frame.sequence_number
            || checkpoint.response_id != frame.response_id
        {
            return Err(ExecutionError::Corrupt);
        }
        let provider = target.launch.provider.slug.as_str();
        let Some(verifier) = self.execution_verifiers.verifier(provider) else {
            return Ok(());
        };
        let scope = ExecutionScope {
            provider: provider.into(),
            channel_record_id: plan.identity.record_id.to_string(),
            event_id: plan.identity.event_id.to_string(),
            consumer_id: target.launch.consumer.to_string(),
            response_id: frame.response_id.clone(),
        };
        let Some(attribution) = verified_change(verifier, &scope, &frame.event)
            .and_then(|change| change.execution_attribution)
        else {
            return Ok(());
        };
        let mut next = lease.state.clone();
        next.delivery
            .as_mut()
            .ok_or(ExecutionError::Corrupt)?
            .execution_attribution = Some(attribution);
        self.commit(lease, next).await
    }

    /// Retry Activity independently once the delivery cursor is acknowledged.
    /// An unavailable projection never returns the terminal frame to delivery.
    pub(super) async fn project_execution(
        &self,
        identity: &EventIdentity,
        consumer: &str,
        lease: &Lease,
    ) -> Result<(), ExecutionError> {
        let Some(activity) = &self.activity else {
            return Ok(());
        };
        let Some(checkpoint) = &lease.state.delivery else {
            return Ok(());
        };
        let Some(attribution) = checkpoint.execution_attribution.clone() else {
            return Ok(());
        };
        let super::Phase::Accepted {
            response_id,
            durable_cursor,
        } = &lease.state.phase
        else {
            return Err(ExecutionError::Corrupt);
        };
        if response_id != &checkpoint.response_id || *durable_cursor != Some(checkpoint.cursor) {
            return Err(ExecutionError::Corrupt);
        }
        let change = crate::activity::Change::execution_attribution(consumer, attribution);
        tokio::time::timeout(
            Duration::from_secs(3),
            activity.change(
                identity.record_id.as_str(),
                identity.event_id.as_str(),
                &change,
            ),
        )
        .await
        .map_err(|_| ExecutionError::Unavailable)?
        .map_err(|_| ExecutionError::Unavailable)
    }
}
