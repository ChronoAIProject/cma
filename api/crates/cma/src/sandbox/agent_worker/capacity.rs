//! Settle capacity failures and exact marked cleanup retries under the same fences.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{
    agent_cleanup_capacity_retry_hint, plan_agent_cleanup_capacity_retry,
    plan_agent_log_capacity_failure, plan_agent_marked_cleanup_capacity_retry, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentWorkStep};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};

use super::AgentWorker;

#[path = "capacity_recovery.rs"]
pub(crate) mod recovery;

#[path = "recovery_commands.rs"]
pub(crate) mod commands;

#[path = "capacity_restart.rs"]
pub(crate) mod reconnect;

impl AgentWorker {
    pub(crate) fn capacity_recovery_commands(
        self: std::sync::Arc<Self>,
        service: std::sync::Arc<crate::store::agent_capacity_recovery::RecoveryService>,
    ) -> std::sync::Arc<
        dyn layer2_core::sandbox::agent_capacity_recovery::commands::CapacityRecoveryCommands,
    > {
        self.install_capacity_restart(service.clone());
        std::sync::Arc::new(commands::AuthorizedRecoveryCommands::new(service, self))
    }

    /// Background entry for an exact already-published recovery operation.
    /// The service proves fresh receipt-bound personal authority for every step;
    /// discovery metadata alone never grants this permission.
    pub(crate) async fn run_accepted_capacity_recovery(
        &self,
        service: &crate::store::agent_capacity_recovery::RecoveryService,
        request: &layer2_core::sandbox::agent_capacity_recovery::state::RecoveryRequest,
    ) -> Result<
        layer2_core::sandbox::agent_capacity_recovery::ports::CommitOutcome<
            layer2_core::sandbox::agent_capacity_recovery::state::RecoveryWork,
        >,
        layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError,
    > {
        let runtime = recovery::RestrictedRecoveryRuntime::new(
            self.dependencies.adapter.clone(),
            self.dependencies.registry.clone(),
            self.dependencies.updates.clone(),
        );
        service
            .run_accepted(&runtime, request, &self.config.holder)
            .await
    }

    /// Authenticated accepted-work entry for shared composition. The caller is
    /// reauthorized by the service on every store operation; a discovery row
    /// must never be converted into a fabricated owner identity.
    pub(crate) async fn run_capacity_recovery(
        &self,
        service: &crate::store::agent_capacity_recovery::RecoveryService,
        caller: &layer2_core::Caller,
        request: &layer2_core::sandbox::agent_capacity_recovery::state::RecoveryRequest,
    ) -> Result<
        layer2_core::sandbox::agent_capacity_recovery::ports::CommitOutcome<
            layer2_core::sandbox::agent_capacity_recovery::state::RecoveryWork,
        >,
        layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError,
    > {
        let runtime = recovery::RestrictedRecoveryRuntime::new(
            self.dependencies.adapter.clone(),
            self.dependencies.registry.clone(),
            self.dependencies.updates.clone(),
        );
        service
            .run_one(&runtime, caller, request, &self.config.holder)
            .await
    }

    /// Select only from a failed-work retry. A point-tail observation is a hint;
    /// commit_agent_work still validates the complete history in its transaction.
    pub(super) async fn retry_marked_capacity_at(
        &self,
        agent: &Agent,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        if !agent_cleanup_capacity_retry_hint(agent) {
            return Ok(false);
        }
        let started = tokio::time::Instant::now();
        let fence = super::support::claimed_fence(agent, &self.config.holder)?;
        let last = self
            .dependencies
            .store
            .agent_log_entry(&agent.id, agent.conversation.last_seq)
            .await?;
        let elapsed = started.elapsed();
        let Some(mutation) = plan_agent_marked_cleanup_capacity_retry(
            agent,
            last.as_ref(),
            &self.config.holder,
            fence,
            &rfc3339_at(now + elapsed),
            &rfc3339_at(ready_at + elapsed),
        )?
        else {
            return Ok(false);
        };
        // Propagate conflicts and unknown commits unchanged. Neither authorizes
        // another proposal or replay of the failed external operation.
        self.dependencies.store.commit_agent_work(mutation).await?;
        Ok(true)
    }

    pub(in crate::sandbox) async fn finish_capacity_attempt(
        &self,
        claimed: &Agent,
        result: Result<(), SandboxStoreError>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if result != Err(SandboxStoreError::AgentLogCapacityExceeded) {
            // Store unavailability and ambiguous commits never establish abort.
            return result;
        }
        let started = tokio::time::Instant::now();
        for attempt in 0..8 {
            let current = self
                .dependencies
                .store
                .get_agent(&claimed.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            let durable_at = seconds_from_rfc3339(&current.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable_at);
            let claim_matches = if crate::store::CLEANUP_RETRY_DEADLINE
                .try_with(|_| ())
                .is_ok()
            {
                super::cleanup_retry::same_cleanup_claim(claimed, &current, &self.config.holder, at)
            } else {
                same_capacity_claim(claimed, &current, &self.config.holder, at)
            };
            if !claim_matches {
                return Err(SandboxStoreError::Conflict);
            }
            let fence = current.work.lease.as_ref().expect("claim checked").fence;
            let cleanup = matches!(
                current.work.step,
                AgentWorkStep::FenceInteraction
                    | AgentWorkStep::CloseProcess
                    | AgentWorkStep::PublishStopped
                    | AgentWorkStep::EnsureProcessClosed
                    | AgentWorkStep::RemoveHome
                    | AgentWorkStep::RemoveWorktree
                    | AgentWorkStep::PublishDeleted
            );
            let failure = super::support::WorkerFailure::retryable(
                "agent_log_capacity",
                "agent log capacity requires recovery",
            );
            let decision = self.retry_decision(&current, &failure, self.config.retry_delay);
            let telemetry = (cleanup && self.config.work_retry_policy_enabled).then(|| {
                crate::sandbox::work_retry::RetryTelemetry::agent(
                    &current,
                    failure.retry_kind(),
                    decision,
                    at + decision.delay,
                )
            });
            let mutation = if cleanup {
                // This is only a planning observation. The fenced commit below
                // validates the complete log and this exact tail again, so do
                // not scan the whole history just to select its final entry.
                let last = self
                    .dependencies
                    .store
                    .agent_log_entry(&current.id, current.conversation.last_seq)
                    .await?;
                plan_agent_cleanup_capacity_retry(
                    &current,
                    last.as_ref(),
                    &self.config.holder,
                    fence,
                    &rfc3339_at(at),
                    &rfc3339_at(at + decision.delay),
                )?
            } else {
                let worktree = self
                    .dependencies
                    .store
                    .get_worktree(&current.worktree_id)
                    .await?
                    .ok_or(SandboxStoreError::NotFound)?;
                plan_agent_log_capacity_failure(
                    &current,
                    &worktree,
                    &self.config.holder,
                    fence,
                    &rfc3339_at(at),
                )?
            };
            match self.dependencies.store.commit_agent_work(mutation).await {
                Err(SandboxStoreError::Conflict) if attempt < 7 => tokio::task::yield_now().await,
                Ok(()) => {
                    if let Some(telemetry) = telemetry {
                        telemetry.published();
                    }
                    if !cleanup {
                        self.dependencies.registry.fence(
                            &current.id,
                            current.conversation.runtime_generation,
                            fence,
                        );
                    }
                    tracing::warn!(agent_id = %current.id, step = ?current.work.step, cleanup_retry = cleanup,
                        "Agent log capacity failure settled");
                    return Ok(());
                }
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn same_capacity_claim(claimed: &Agent, current: &Agent, holder: &str, at: SystemTime) -> bool {
    current.revision >= claimed.revision
        && current.conversation.last_seq >= claimed.conversation.last_seq
        && current.id == claimed.id
        && current.owner == claimed.owner
        && current.sandbox_id == claimed.sandbox_id
        && current.workspace_id == claimed.workspace_id
        && current.worktree_id == claimed.worktree_id
        && current.profile == claimed.profile
        && current.created_at == claimed.created_at
        && current.creation_key == claimed.creation_key
        && current.creation_fingerprint == claimed.creation_fingerprint
        && current.origin == claimed.origin
        && current.conversation.conversation_id == claimed.conversation.conversation_id
        && current.conversation.runtime_generation == claimed.conversation.runtime_generation
        && current.work.step == claimed.work.step
        && current.work.next_fence == claimed.work.next_fence
        && current.work.lease == claimed.work.lease
        && current.requested_stop_reason == claimed.requested_stop_reason
        && current.stopped_at == claimed.stopped_at
        && current.deleted_at == claimed.deleted_at
        && (current.status == claimed.status
            || claimed.work.step == AgentWorkStep::DispatchFirstMessage
                && claimed.status.is_live()
                && current.status.is_live())
        && current.work.lease.as_ref().is_some_and(|lease| {
            lease.holder == holder
                && lease.fence > 0
                && seconds_from_rfc3339(&lease.expires_at).is_some_and(|expires| {
                    at < SystemTime::UNIX_EPOCH + Duration::from_secs(expires)
                })
        })
}
