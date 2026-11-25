//! Requeue an unchanged cleanup claim after a preflight refusal or a definite abort.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep};

use super::{cleanup::CleanupPreflightError, support::WorkerFailure, AgentWorker};
use crate::store::CLEANUP_RETRY_DEADLINE;

impl AgentWorker {
    pub(super) async fn finish_cleanup_preflight(
        &self,
        claimed: &Agent,
        error: CleanupPreflightError,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let failure = match error {
            CleanupPreflightError::Read(SandboxStoreError::Unavailable(_)) => {
                WorkerFailure::retryable(
                    "agent_cleanup_preflight_unavailable",
                    "cleanup could not read current parent records",
                )
            }
            CleanupPreflightError::ArchivePending => WorkerFailure::retryable(
                "agent_cleanup_archive_pending",
                "cleanup is waiting for the retirement archive to be verified",
            ),
            CleanupPreflightError::Read(error) => return Err(error),
        };
        let started = tokio::time::Instant::now();
        // The scheduler's timestamp can predate ownership and admission I/O.
        // Never translate that delay into extra time on the stored claim.
        let cutoff = now.max(SystemTime::now());
        let remaining = claimed
            .work
            .lease
            .as_ref()
            .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
            .and_then(|seconds| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .and_then(|expires| expires.duration_since(cutoff).ok())
            .ok_or(SandboxStoreError::Conflict)?;
        let deadline = started
            .checked_add(remaining)
            .ok_or(SandboxStoreError::Conflict)?;
        CLEANUP_RETRY_DEADLINE
            .scope(deadline, async {
                let result = self
                    .requeue_cleanup(claimed, failure, now, self.config.retry_delay)
                    .await;
                self.finish_capacity_attempt(claimed, result, now + started.elapsed())
                    .await
            })
            .await
    }

    pub(in crate::sandbox) async fn finish_cleanup_attempt(
        &self,
        claimed: &Agent,
        result: Result<(), SandboxStoreError>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if result != Err(SandboxStoreError::Conflict) || !cleanup_step(claimed.work.step) {
            // Unknown commits do not establish abort. Never release their
            // authority or replace their existing reconciliation path.
            return result;
        }
        let failure = WorkerFailure::retryable(
            "agent_cleanup_publication_conflict",
            "cleanup publication conflicted; retrying the unchanged cleanup claim",
        );
        self.requeue_cleanup(claimed, failure, now, Duration::from_secs(1))
            .await
    }

    async fn requeue_cleanup(
        &self,
        claimed: &Agent,
        failure: WorkerFailure,
        now: SystemTime,
        base_delay: Duration,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        for attempt in 0..8 {
            if CLEANUP_RETRY_DEADLINE
                .try_with(|deadline| tokio::time::Instant::now() >= *deadline)
                .unwrap_or(false)
            {
                return Err(SandboxStoreError::Conflict);
            }
            let current = self
                .dependencies
                .store
                .get_agent(&claimed.id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            let durable_at = layer2_core::seconds_from_rfc3339(&current.updated_at)
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid agent update time".into()))?;
            let at = (now + started.elapsed()).max(durable_at);
            if !same_cleanup_claim(claimed, &current, &self.config.holder, at) {
                return Err(SandboxStoreError::Conflict);
            }
            let delay = self.retry_decision(&current, &failure, base_delay).delay;
            // Only schedule the original durable step. Its next worker owns
            // the fresh fence and reuses stable external operation receipts.
            match self
                .retry_at(current, failure.clone(), at, at + delay)
                .await
            {
                Err(SandboxStoreError::Conflict) if attempt < 7 => tokio::task::yield_now().await,
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn cleanup_step(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
            | AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

pub(in crate::sandbox) fn same_cleanup_claim(
    claimed: &Agent,
    current: &Agent,
    holder: &str,
    now: SystemTime,
) -> bool {
    if !cleanup_step(claimed.work.step)
        || current.revision < claimed.revision
        || current.conversation.last_seq < claimed.conversation.last_seq
        || current.work != claimed.work
        || current.work.lease.as_ref().is_none_or(|lease| {
            lease.holder != holder
                || lease.fence == 0
                || layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none_or(|seconds| {
                    now >= SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
                })
        })
    {
        return false;
    }
    // Adopt concurrent display/log metadata only. Work, stop authority,
    // settings, parent bindings, runtime and admitted input must stay exact.
    let mut normalized = current.clone();
    normalized.revision = claimed.revision;
    normalized.updated_at = claimed.updated_at.clone();
    normalized.title = claimed.title.clone();
    normalized.conversation.last_seq = claimed.conversation.last_seq;
    normalized == *claimed
}
