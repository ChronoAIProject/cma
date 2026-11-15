//! Retry a known-aborted startup intent without replaying a native operation.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep, SandboxStatus};
use layer2_core::seconds_from_rfc3339;

use super::startup_publication::{holds_startup_authority, only_startup_metadata_changed};
use super::support::WorkerFailure;
use super::AgentWorker;

const STARTUP_RETRY_DELAY: Duration = Duration::from_secs(3);
const MAX_STARTUP_CLAIM_RETRIES: u32 = 3;

impl AgentWorker {
    pub(in crate::sandbox) async fn finish_startup_attempt(
        &self,
        claimed: &Agent,
        result: Result<(), SandboxStoreError>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if result != Err(SandboxStoreError::Conflict)
            || !matches!(
                claimed.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
            )
            || claimed.work.attempts >= MAX_STARTUP_CLAIM_RETRIES
            || claimed
                .work
                .cleanup_debt
                .iter()
                .any(|debt| debt.step == claimed.work.step)
        {
            return result;
        }
        let started = tokio::time::Instant::now();
        for attempt in 0..4 {
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
            // Each native startup effect requires a durable intent before dispatch.
            // Exact unchanged work plus no intent for this step proves that this
            // attempt never reached it. A new claim, paired intent, completion,
            // Stop or runtime change must retain its own recovery authority.
            if !holds_startup_authority(&current, &self.config.holder, at)
                || (current != *claimed && !only_startup_metadata_changed(claimed, &current))
            {
                return Err(SandboxStoreError::Conflict);
            }
            let parent = self
                .dependencies
                .store
                .get_sandbox(&current.sandbox_id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            if parent.owner != current.owner
                || parent.status != SandboxStatus::Active
                || !parent.credential.delivered
                || parent.runtime.is_none()
                || parent.lifecycle.checkpoint_capture.is_some()
            {
                return Err(SandboxStoreError::Conflict);
            }
            let failure = WorkerFailure::retryable(
                "agent_startup_publication_conflict",
                "startup intent publication aborted before native dispatch; retrying shortly",
            );
            // This existing store API CASes revision, holder, fence and ownership.
            // An unknown result is propagated; it never authorizes another write.
            match self
                .retry_at(current, failure, at, at + STARTUP_RETRY_DELAY)
                .await
            {
                Err(SandboxStoreError::Conflict) if attempt < 3 => tokio::task::yield_now().await,
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

/// Contention after a completed effect stays inside its publication closure.
/// Eight attempts remain the cap; two pauses give other writers time to finish.
pub(super) async fn publication_backoff(attempt: usize) {
    if matches!(attempt, 3 | 5) {
        tokio::time::sleep(STARTUP_RETRY_DELAY).await;
    } else {
        tokio::task::yield_now().await;
    }
}
