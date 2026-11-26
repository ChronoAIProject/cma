//! Pending-target preparation and the Activity reason it leaves on failure.
use super::*;

impl Dispatcher {
    /// Prepare the binding, conversation and frozen body of a pending target.
    pub(super) async fn prepare_pending(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
        current: Arc<ResolvedChannel>,
    ) -> Result<(), ExecutionError> {
        let generation = if plan.prepares_binding(target) {
            self.prepare_binding(plan, target, lease).await?
        } else {
            None
        };
        let current = if plan.prepares_binding(target) {
            let current = self.current(plan, target).await?;
            self.journal.renew(lease).await?;
            self.journal.guard(lease).await?;
            current
        } else {
            current
        };
        let body = self
            .port
            .prepare_at_generation(plan, target, &current, generation)
            .await?;
        validate_delivery(&body, &current, target)?;
        self.journal.freeze(lease, plan, target, &body).await
    }

    /// A target that cannot be prepared stays queued; name why on its activity
    /// so the owner sees more than an unexplained queue. Only this worker's own
    /// failures are named: a lost lease belongs to the worker that took over.
    pub(super) async fn observe_preparation_failure(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        error: &ExecutionError,
    ) {
        let Some(code) = preparation_failure_code(error) else {
            return;
        };
        let mut change = crate::activity::Change::state(
            target.launch.consumer.as_str(),
            crate::activity::ConsumerState::Queued,
            None,
        );
        change.last_non_admission_code = Some(code.into());
        self.journal.observe(&plan.identity, change).await;
    }
}

/// The closed activity code naming why a target could not be prepared.
pub(in crate::turns::routed) fn preparation_failure_code(
    error: &ExecutionError,
) -> Option<&'static str> {
    Some(match error {
        ExecutionError::LostLease => return None,
        ExecutionError::Busy => "preparation_provider_busy",
        ExecutionError::Unavailable => "preparation_provider_unavailable",
        ExecutionError::Uncertain => "preparation_outcome_uncertain",
        ExecutionError::AuthorityChanged => "preparation_authority_changed",
        ExecutionError::Expired => "preparation_event_expired",
        ExecutionError::Unsupported => "preparation_unsupported",
        ExecutionError::Invalid
        | ExecutionError::Corrupt
        | ExecutionError::IdempotencyConflict
        | ExecutionError::OperationLimit
        | ExecutionError::Abandoned => "preparation_failed",
    })
}
