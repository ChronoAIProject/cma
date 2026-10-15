use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentWorkStep, FirstMessageDelivery, FirstMessageDispatchOutcome,
};
use layer2_core::seconds_from_rfc3339;

use super::AgentWorker;

impl AgentWorker {
    pub(in crate::sandbox) async fn finish_dispatch_publication(
        &self,
        current: Agent,
        at: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.progress(
            current,
            AgentWorkStep::None,
            at,
            None,
            Vec::new(),
            None,
            None,
        )
        .await
    }

    // Retry only publication: the already admitted prompt must never be sent again.
    pub(in crate::sandbox) async fn retry_dispatch_publication<F, Fut>(
        &self,
        claimed: &Agent,
        outcome: &FirstMessageDispatchOutcome,
        now: SystemTime,
        mut publish: F,
    ) -> Result<(), SandboxStoreError>
    where
        F: FnMut(Agent, SystemTime) -> Fut,
        Fut: Future<Output = Result<(), SandboxStoreError>>,
    {
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
            let observed_at = now + started.elapsed();
            let at = observed_at.max(durable_at);
            if !same_dispatch_claim(claimed, &current, &self.config.holder, observed_at)
                || !completed_dispatch(&current, outcome)
            {
                return Err(SandboxStoreError::Conflict);
            }
            match publish(current, at).await {
                Err(SandboxStoreError::Conflict) if attempt < 7 => {
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn same_dispatch_claim(claimed: &Agent, current: &Agent, holder: &str, now: SystemTime) -> bool {
    claimed.work.step == AgentWorkStep::DispatchFirstMessage
        && current.work == claimed.work
        && current.revision >= claimed.revision
        && current.conversation.last_seq >= claimed.conversation.last_seq
        && current.id == claimed.id
        && current.owner == claimed.owner
        && current.sandbox_id == claimed.sandbox_id
        && current.workspace_id == claimed.workspace_id
        && current.worktree_id == claimed.worktree_id
        && current.conversation.conversation_id == claimed.conversation.conversation_id
        && current.conversation.runtime_generation == claimed.conversation.runtime_generation
        && current.status.is_live()
        && current.requested_stop_reason.is_none()
        && current.deleted_at.is_none()
        && current.work.lease.as_ref().is_some_and(|lease| {
            lease.holder == holder
                && lease.fence > 0
                && seconds_from_rfc3339(&lease.expires_at).is_some_and(|seconds| {
                    now < SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
                })
        })
}

fn completed_dispatch(current: &Agent, outcome: &FirstMessageDispatchOutcome) -> bool {
    let expected = match outcome {
        FirstMessageDispatchOutcome::NotConfigured => {
            return current.conversation.first_message.is_none();
        }
        FirstMessageDispatchOutcome::CommonCommand(expected)
        | FirstMessageDispatchOutcome::Accepted(expected)
        | FirstMessageDispatchOutcome::RecoveryRequired(expected) => expected,
        _ => return false,
    };
    let Some(actual) = current.conversation.first_message.as_ref() else {
        return false;
    };
    let same_attempt = actual.origin_key == expected.origin_key
        && actual.text == expected.text
        && actual.payload_sha256 == expected.payload_sha256
        && actual.queued_at == expected.queued_at
        && actual.attempt == expected.attempt
        && actual.recovery_generation == expected.recovery_generation
        && actual.fence_sequence == expected.fence_sequence
        && actual.dispatch_generation == expected.dispatch_generation
        && actual.turn_sequence == expected.turn_sequence
        && actual.common_binding == expected.common_binding;
    same_attempt
        && match outcome {
            FirstMessageDispatchOutcome::CommonCommand(_) => actual.common_binding.is_some(),
            FirstMessageDispatchOutcome::Accepted(_) => matches!(
                actual.state,
                FirstMessageDelivery::WriteConfirmed
                    | FirstMessageDelivery::ExecutionObserved
                    | FirstMessageDelivery::Completed
                    | FirstMessageDelivery::Failed
            ),
            FirstMessageDispatchOutcome::RecoveryRequired(_) => {
                actual.state == FirstMessageDelivery::RecoveryRequired
            }
            _ => false,
        }
}
