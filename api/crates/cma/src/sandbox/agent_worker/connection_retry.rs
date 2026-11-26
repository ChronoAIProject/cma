//! Do not strand a newly claimed connection lease after known-aborted publication.
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentWorkPublicationParents, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentWorkStep, FirstMessageRecord};

use super::support::WorkerFailure;
use super::AgentWorker;
use crate::sandbox::live_agents::LiveAgentAttachment;

impl AgentWorker {
    pub(in crate::sandbox) async fn finish_connection_publication(
        &self,
        agent: Agent,
        attachment: &LiveAgentAttachment,
        parents: Option<AgentWorkPublicationParents>,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        match self
            .try_finish_connection_publication(agent.clone(), attachment, parents, now)
            .await
        {
            Err(SandboxStoreError::Conflict) => {
                self.retry_connection_publication(&agent, now + started.elapsed())
                    .await
            }
            // In particular, unknown commit/store availability failures do not
            // prove that publication aborted and cannot release this lease.
            result => result,
        }
    }

    async fn retry_connection_publication(
        &self,
        claimed: &Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let failure = WorkerFailure::retryable(
            "agent_connection_publication_conflict",
            "connection publication conflicted; retrying the unchanged connection claim",
        );
        let delay = self
            .retry_decision(claimed, &failure, Duration::from_secs(1))
            .delay;
        self.retry_connection_with_failure(claimed, failure, now, delay)
            .await
    }

    pub(super) async fn retry_connection_failure(
        &self,
        claimed: &Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let delay = self.failure_retry_delay(claimed, &failure);
        self.retry_connection_with_failure(claimed, failure, now, delay)
            .await
    }

    async fn retry_connection_with_failure(
        &self,
        claimed: &Agent,
        failure: WorkerFailure,
        now: SystemTime,
        delay: Duration,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        for attempt in 0..8 {
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
            if !same_connection_claim(claimed, &current, &self.config.holder, at)
                && !super::connection_recovery_progress::same_recovery_claim(
                    claimed,
                    &current,
                    &self.config.holder,
                    at,
                )
            {
                return Err(SandboxStoreError::Conflict);
            }
            match self
                .retry_at(current, failure.clone(), at, at + delay)
                .await
            {
                Err(SandboxStoreError::Conflict) if attempt < 7 => {
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

pub(super) fn same_connection_claim(
    claimed: &Agent,
    current: &Agent,
    holder: &str,
    now: SystemTime,
) -> bool {
    if claimed.work.step != AgentWorkStep::PublishConnection
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
    if current.conversation.first_message != claimed.conversation.first_message {
        let Some(first) = current.conversation.first_message.as_ref() else {
            return false;
        };
        if claimed.conversation.first_message.is_some()
            || FirstMessageRecord::new(&claimed.creation_key, &first.text, &first.queued_at)
                .as_ref()
                != Ok(first)
        {
            return false;
        }
    }
    // Only metadata that can legitimately arrive while the connection is
    // published is adopted. Work/lease, settings, target, parent binding,
    // active turn, pending operations and existing queued input stay exact.
    let mut normalized = current.clone();
    normalized.revision = claimed.revision;
    normalized.updated_at = claimed.updated_at.clone();
    normalized.title = claimed.title.clone();
    normalized.conversation.last_seq = claimed.conversation.last_seq;
    normalized.conversation.first_message = claimed.conversation.first_message.clone();
    normalized == *claimed
}
