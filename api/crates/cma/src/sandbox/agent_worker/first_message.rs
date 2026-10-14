use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentOperationError, AgentStatus, AgentWorkAuthority, AgentWorkStep,
    FirstMessageDelivery, FirstMessageDispatchOutcome,
};
use layer2_core::seconds_from_rfc3339;

use super::startup_support::{live_target, registry_failure};
use super::support::{claimed_fence, WorkerFailure};
use super::AgentWorker;

impl AgentWorker {
    pub(in crate::sandbox) async fn dispatch_first_message(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Err(failure) = self.ensure_dispatch_connection(&agent).await {
            return self.handle_failure(agent, failure, now).await;
        }
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let authority =
            AgentWorkAuthority::new(self.config.holder.clone(), fence).map_err(dispatch_error)?;
        let started = tokio::time::Instant::now();
        let outcome = self
            .dependencies
            .operations
            .dispatch_queued_first_message(&agent.id, authority.clone())
            .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(AgentOperationError::Conflict) => {
                return self
                    .retry_unadmitted_first_message(&agent, &authority, now + started.elapsed())
                    .await;
            }
            Err(error) => return Err(dispatch_error(error)),
        };
        match outcome {
            FirstMessageDispatchOutcome::CommonCommand(_)
            | FirstMessageDispatchOutcome::NotConfigured
            | FirstMessageDispatchOutcome::Accepted(_)
            | FirstMessageDispatchOutcome::RecoveryRequired(_) => {
                let current = self.current_dispatch(&agent, &authority).await?;
                let now = publication_time(now, &current)?;
                self.progress(
                    current,
                    AgentWorkStep::None,
                    now,
                    None,
                    Vec::new(),
                    None,
                    None,
                )
                .await
            }
            FirstMessageDispatchOutcome::Retryable(_) => {
                let current = self.current_dispatch(&agent, &authority).await?;
                let now = publication_time(now, &current)?;
                self.handle_failure(
                    current,
                    WorkerFailure::retryable(
                        "agent_first_message_pre_write_failure",
                        "the runtime writer rejected the first message before queue admission",
                    ),
                    now,
                )
                .await
            }
            FirstMessageDispatchOutcome::InProgress(_) => Ok(()),
        }
    }

    async fn retry_unadmitted_first_message(
        &self,
        original: &Agent,
        authority: &AgentWorkAuthority,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // A fresh exact-fence observation is required: a dispatch fence or any
        // write evidence must never be reset into a second prompt attempt.
        let current = self.current_dispatch(original, authority).await?;
        let now = publication_time(now, &current)?;
        if current.status != AgentStatus::Ready
            || current.conversation.active_turn.is_some()
            || current.conversation.first_message != original.conversation.first_message
            || current
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at <= layer2_core::rfc3339_at(now))
            || !current
                .conversation
                .first_message
                .as_ref()
                .is_some_and(|record| record.state == FirstMessageDelivery::Queued)
        {
            return Err(SandboxStoreError::Conflict);
        }
        self.retry_at(
            current,
            WorkerFailure::retryable(
                "agent_first_message_admission_conflict",
                "first-message admission conflicted before dispatch; retrying fresh state",
            ),
            now,
            now + Duration::from_secs(1),
        )
        .await
    }

    async fn ensure_dispatch_connection(&self, agent: &Agent) -> Result<(), WorkerFailure> {
        let target = live_target(agent)?;
        if self
            .dependencies
            .registry
            .published_target_is_attached(&target)
        {
            return Ok(());
        }
        let context = self.connection_context(agent).await?;
        let attachment = self.reattach(agent, &context, &target).await?;
        if let Err(error) = self.dependencies.registry.publish_and_activate(&attachment) {
            self.dependencies.registry.rollback(&attachment);
            return Err(registry_failure(error));
        }
        Ok(())
    }

    async fn current_dispatch(
        &self,
        original: &Agent,
        authority: &AgentWorkAuthority,
    ) -> Result<Agent, SandboxStoreError> {
        let current = self
            .dependencies
            .store
            .get_agent(&original.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.work.step != AgentWorkStep::DispatchFirstMessage
            || !current.work.owns(&authority.holder, authority.fence)
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(current)
    }
}

fn publication_time(now: SystemTime, current: &Agent) -> Result<SystemTime, SandboxStoreError> {
    let seconds = seconds_from_rfc3339(&current.updated_at)
        .ok_or_else(|| SandboxStoreError::Corrupt("agent update timestamp is invalid".into()))?;
    let durable = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
    Ok(std::cmp::max(now, durable))
}

fn dispatch_error(error: AgentOperationError) -> SandboxStoreError {
    match error {
        AgentOperationError::NotFound => SandboxStoreError::NotFound,
        AgentOperationError::StaleWorkAuthority
        | AgentOperationError::Conflict
        | AgentOperationError::CheckpointInProgress => SandboxStoreError::Conflict,
        AgentOperationError::LogCapacityExceeded => SandboxStoreError::AgentLogCapacityExceeded,
        AgentOperationError::Corrupt(detail) => SandboxStoreError::Corrupt(detail),
        AgentOperationError::Unavailable(detail)
        | AgentOperationError::RuntimeUnavailable(detail)
        | AgentOperationError::OutcomeUnavailable(detail) => SandboxStoreError::Unavailable(detail),
        unexpected => SandboxStoreError::Corrupt(format!(
            "first-message dispatcher returned an invalid lifecycle outcome: {unexpected}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_busy_preserves_durable_first_message_retry() {
        assert_eq!(
            dispatch_error(AgentOperationError::CheckpointInProgress),
            SandboxStoreError::Conflict
        );
    }
}
