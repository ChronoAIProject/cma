use super::{FirstMessageDelivery, FirstMessageFailure, FirstMessageRecord, SandboxModelError};

impl FirstMessageRecord {
    pub fn refuse_budget(
        &mut self,
        reason: crate::budget::BudgetError,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.refuse_before_dispatch(reason.code(), reason.guidance(), at)
    }

    pub fn refuse_session_capacity(&mut self, at: String) -> Result<(), SandboxModelError> {
        let kind = crate::admission::CapacityKind::ActiveSessions;
        self.refuse_before_dispatch(kind.code(), kind.guidance(), at)
    }

    fn refuse_before_dispatch(
        &mut self,
        code: &str,
        guidance: &str,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        if self.state != FirstMessageDelivery::Queued || !self.unattempted() {
            return Err(SandboxModelError::invalid_value(
                "first_message.admission_refusal",
            ));
        }
        self.state = FirstMessageDelivery::Failed;
        self.failed_at = Some(at.clone());
        self.failure = Some(FirstMessageFailure::new(code, guidance, false, at)?);
        self.validate()
    }
    /// Close a never-dispatched creation input when conversation startup ends.
    /// This does not authorize retry on the failed Agent or invent a turn.
    pub fn fail_startup(&mut self, at: String) -> Result<(), SandboxModelError> {
        self.validate()?;
        // A bound creation command can finish before startup does. Keep its
        // terminal evidence while allowing the Agent's failure to publish.
        if self.state == FirstMessageDelivery::Failed && self.unattempted() {
            return Ok(());
        }
        if self.state != FirstMessageDelivery::Queued || !self.unattempted() {
            return Err(SandboxModelError::invalid_value(
                "first_message.startup_failure",
            ));
        }
        self.state = FirstMessageDelivery::Failed;
        self.failed_at = Some(at.clone());
        self.failure = Some(FirstMessageFailure::new(
            "agent_startup_failed",
            "Conversation startup failed before dispatch. Inspect the Agent failure; after repair, create a replacement Agent.",
            false,
            at,
        )?);
        self.validate()
    }

    pub(super) fn unattempted_startup_failure(&self) -> bool {
        self.unattempted()
            && self.failure.as_ref().is_some_and(|failure| {
                matches!(
                    failure.code.as_str(),
                    "agent_startup_failed"
                        | "owner_budget_exhausted"
                        | "turn_budget_exhausted"
                        | "budget_unavailable"
                        | "platform_budget_exhausted"
                        | "session_capacity_unavailable"
                ) && !failure.retryable
                    && self.failed_at.as_ref() == Some(&failure.occurred_at)
            })
    }

    fn unattempted(&self) -> bool {
        self.attempt == 0
            && self.recovery_generation == 0
            && self.fence_sequence == 0
            && self.dispatch_generation == 0
            && self.turn_sequence.is_none()
            && self.dispatch_fenced_at.is_none()
            && self.write_confirmed_at.is_none()
            && self.execution_observed_at.is_none()
    }
}
