use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, AgentOperationError, AgentStatus, QueuedMessageState, Sandbox, SandboxEventKind,
    SandboxWorkStep, QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};

use super::super::support::WorkerFailure;
use super::super::SandboxWorker;
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    #[cfg(test)]
    pub(in crate::sandbox) async fn complete_lifecycle_queue_for_test(
        &self,
        sandbox: Sandbox,
        agents: Vec<Agent>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.complete_lifecycle_queue(sandbox, agents, now).await
    }

    pub(super) async fn deliver_lifecycle_queue(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let agents = self.lifecycle_agents(&sandbox).await?;
        let fenced = agents
            .iter()
            .filter(|agent| sandbox.restores_agent(agent))
            .find_map(|agent| {
                agent
                    .conversation
                    .queued_messages
                    .iter()
                    .find(|message| {
                        matches!(
                            message.state,
                            QueuedMessageState::DispatchFenced
                                | QueuedMessageState::RecoveryRequired
                        )
                    })
                    .map(|message| {
                        (
                            agent.id.clone(),
                            message.sequence,
                            message.state,
                            message.dispatch_fenced_at.clone(),
                        )
                    })
            });
        if let Some((agent_id, sequence, state, fenced_at)) = fenced {
            if state == QueuedMessageState::DispatchFenced
                && dispatch_grace_elapsed(fenced_at.as_deref(), now)
            {
                let stamp = rfc3339_at(now);
                let failure =
                    WorkerFailure::waiting("lifecycle queued messages still require settlement");
                let decision = self.retry_decision(&sandbox, &failure, self.config.retry_delay);
                let telemetry = self.retry_telemetry(&sandbox, &failure, decision, now);
                let recovery = self
                    .dependencies
                    .operations
                    .recover_stale_lifecycle_queue_dispatch(
                        &sandbox,
                        &agent_id,
                        sequence,
                        rfc3339_at(now + decision.delay),
                        stamp,
                    )
                    .await;
                return match recovery {
                    Ok(()) => {
                        if let Some(telemetry) = telemetry {
                            telemetry.published();
                        }
                        Ok(())
                    }
                    Err(AgentOperationError::Conflict) => {
                        self.retry_queue_phase(sandbox, now).await
                    }
                    Err(
                        AgentOperationError::Unavailable(_)
                        | AgentOperationError::OutcomeUnavailable(_),
                    ) => Ok(()),
                    Err(error) => {
                        tracing::warn!(sandbox_id = %sandbox.id, agent_id = %agent_id, error = ?error, "stale lifecycle queue fence could not be retired");
                        self.retry_queue_phase(sandbox, now).await
                    }
                };
            }
            tracing::debug!(agent_id = %agent_id, "lifecycle queue is waiting for dispatch settlement");
            return self.retry_queue_phase(sandbox, now).await;
        }
        if let Some(agent) = agents
            .iter()
            .filter(|agent| sandbox.restores_agent(agent))
            .find(|agent| !queue_agent_is_quiet(agent))
        {
            tracing::debug!(agent_id = %agent.id, "lifecycle queue is waiting for agent settlement");
            return self.retry_queue_settlement(sandbox, now).await;
        }

        let pending = agents
            .iter()
            .filter(|agent| sandbox.restores_agent(agent))
            .find_map(|agent| {
                agent
                    .conversation
                    .queued_messages
                    .iter()
                    .find(|message| message.state == QueuedMessageState::Queued)
                    .map(|message| (agent.id.clone(), message.sequence))
            });
        let Some((agent_id, sequence)) = pending else {
            return self.complete_lifecycle_queue(sandbox, agents, now).await;
        };

        let stamp = rfc3339_at(now);
        match self
            .dependencies
            .operations
            .dispatch_lifecycle_queued_message(&sandbox, &agent_id, sequence, stamp)
            .await
        {
            Ok(_) => Ok(()),
            Err(
                AgentOperationError::RuntimeUnavailable(_)
                | AgentOperationError::DeliveryAmbiguous { .. }
                | AgentOperationError::OutcomeUnavailable(_),
            ) => Ok(()),
            Err(error) => {
                tracing::warn!(sandbox_id = %sandbox.id, agent_id = %agent_id, error = ?error, "lifecycle queue dispatch was rejected");
                self.retry_queue_phase(sandbox, now).await
            }
        }
    }

    async fn complete_lifecycle_queue(
        &self,
        sandbox: Sandbox,
        agents: Vec<Agent>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_queued_message_delivery(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        let mutations = agents
            .into_iter()
            .filter(|agent| sandbox.restores_agent(agent))
            .map(|current| prune_delivered_queue(current, &stamp))
            .collect::<Result<Vec<_>, _>>()?;
        let completion = self
            .commit_lifecycle_phase(
                &sandbox,
                next,
                mutations,
                SandboxLifecycleAgentScope::RestoreParticipants,
                stamp,
                vec![SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                }],
            )
            .await;
        match completion {
            // Reopened agents can publish metadata after the queue snapshot. Retry only
            // this internal commit, under the unchanged parent revision and claim.
            Err(SandboxStoreError::Conflict) => {
                Box::pin(self.retry_queue_settlement(sandbox, now)).await
            }
            outcome => outcome,
        }
    }

    async fn retry_queue_settlement(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        // Reopened agents finish their metadata/lease commits independently.
        // Recheck local state promptly without shortening external failure
        // backoff or the grace period for an ambiguous message dispatch.
        self.handle_lifecycle_failure_after(
            sandbox,
            WorkerFailure::waiting("lifecycle queued messages still require settlement"),
            now,
            self.config.retry_delay.min(Duration::from_secs(5)),
        )
        .await
    }

    async fn retry_queue_phase(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_lifecycle_failure(
            sandbox,
            WorkerFailure::waiting("lifecycle queued messages still require settlement"),
            now,
        )
        .await
    }
}

fn queue_agent_is_quiet(agent: &Agent) -> bool {
    agent.status == AgentStatus::Ready
        && agent.conversation.active_turn.is_none()
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|message| message.is_terminal())
        && agent
            .conversation
            .operation
            .as_ref()
            .is_none_or(|operation| !operation.blocks_dispatch())
        && !agent.work.step.is_pending()
        && agent.work.lease.is_none()
        && agent.requested_stop_reason.is_none()
}

fn dispatch_grace_elapsed(fenced_at: Option<&str>, now: SystemTime) -> bool {
    let Some(fenced_at) = fenced_at.and_then(seconds_from_rfc3339) else {
        return false;
    };
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    fenced_at
        .checked_add(QUEUED_MESSAGE_DISPATCH_GRACE_SECONDS)
        .is_some_and(|deadline| now >= deadline)
}

fn prune_delivered_queue(
    current: Agent,
    at: &str,
) -> Result<SandboxLifecycleAgentMutation, SandboxStoreError> {
    if current
        .conversation
        .queued_messages
        .iter()
        .any(|message| message.state != QueuedMessageState::Delivered)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut agent = current.clone();
    agent.conversation.queued_messages.clear();
    agent.revision = agent
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    agent.updated_at = at.into();
    Ok(SandboxLifecycleAgentMutation {
        expected_revision: current.revision,
        agent,
        logs: Vec::new(),
    })
}
