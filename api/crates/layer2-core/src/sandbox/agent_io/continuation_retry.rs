//! Retry only an exact durable proof that the ordered writer performed no IO.
use super::*;
use serde::{Deserialize, Serialize};

pub const SOURCE_DEFERRED_FAILURE: &str = "input_authority_unavailable_before_write";
pub const MAX_DEFERRED_ATTEMPTS: usize = 256;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferredCommandAttempt {
    pub dispatch: CommandDispatch,
    pub fingerprint: String,
    pub failure_cursor: u64,
    pub failed_at: String,
    pub retry_after: String,
}
pub(in crate::sandbox) fn deferred_detail(
    envelope: &AgentCommandEnvelope,
    operation: u64,
    turn: u64,
) -> String {
    use sha2::{Digest, Sha256};
    let proof = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&envelope.continuation).expect("typed provenance"))
    );
    serde_json::to_string(&(
        &envelope.command_id,
        &envelope.request_fingerprint,
        proof,
        operation,
        turn,
    ))
    .expect("typed continuation proof")
}
impl AdmittedAgentCommand {
    pub fn continuation_retry_delay(&self, now: &str) -> std::time::Duration {
        let delay = self
            .work
            .deferred_attempts
            .last()
            .and_then(|attempt| crate::seconds_from_rfc3339(&attempt.retry_after))
            .zip(crate::seconds_from_rfc3339(now))
            .map_or(0, |(until, now)| until.saturating_sub(now));
        std::time::Duration::from_secs(delay)
    }
    pub(super) fn defer_source_attempt(
        &mut self,
        agent: &crate::sandbox::Agent,
        allowed: bool,
        cursor: u64,
        at: &str,
    ) -> Result<(), CommandLedgerError> {
        let now = crate::now_rfc3339();
        let refusal = if !allowed {
            Some(CommandFailureCode::AuthorityRevoked)
        } else if self.queue_expired(&now) {
            Some(CommandFailureCode::QueueTimeout)
        } else if agent.requested_stop_reason.is_some()
            || agent.status != crate::sandbox::AgentStatus::Ready
            || agent.conversation.active_turn.is_some()
        {
            Some(CommandFailureCode::AgentUnavailable)
        } else if self.work.deferred_attempts.len() >= MAX_DEFERRED_ATTEMPTS {
            Some(CommandFailureCode::AuthorityUnavailable)
        } else {
            None
        };
        if let Some(code) = refusal {
            self.work.failure = Some(CommandFailure { code, at: now });
            return Ok(());
        }
        let dispatch = self
            .work
            .dispatch
            .take()
            .ok_or(CommandLedgerError::Corrupt)?;
        let delay = 2_u64
            .pow((self.work.deferred_attempts.len() as u32 + 1).min(8))
            .min(300);
        let failed = crate::seconds_from_rfc3339(at).ok_or(CommandLedgerError::Corrupt)?;
        self.work.deferred_attempts.push(DeferredCommandAttempt {
            dispatch,
            fingerprint: self.receipt.envelope.request_fingerprint.clone(),
            failure_cursor: cursor,
            failed_at: at.into(),
            retry_after: crate::rfc3339_at(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(failed + delay),
            ),
        });
        self.status = AgentResponseStatus::Queued;
        self.work.lease = None;
        self.work.failure = None;
        self.work.observation = None;
        Ok(())
    }
}
pub(super) fn validate_attempts(command: &AdmittedAgentCommand) -> Result<(), CommandLedgerError> {
    let attempts = &command.work.deferred_attempts;
    if attempts.len() > MAX_DEFERRED_ATTEMPTS
        || (!attempts.is_empty() && command.receipt.envelope.continuation.is_none())
    {
        return Err(CommandLedgerError::Corrupt);
    }
    let mut previous = 0;
    for attempt in attempts {
        let dispatch = &attempt.dispatch;
        if dispatch.claim.command_id != command.receipt.envelope.command_id
            || dispatch.claim.agent_id != command.receipt.envelope.agent_id
            || dispatch.claim.fence == 0
            || dispatch.claim.holder.is_empty()
            || dispatch.runtime_generation == 0
            || dispatch.delivery != crate::sandbox::AgentOperationDelivery::DispatchFenced
            || dispatch.operation_sequence <= previous
            || dispatch.turn_sequence != dispatch.operation_sequence + 1
            || attempt.failure_cursor <= dispatch.turn_sequence
            || attempt.fingerprint != command.receipt.envelope.request_fingerprint
            || crate::seconds_from_rfc3339(&dispatch.fenced_at)
                .zip(crate::seconds_from_rfc3339(&attempt.failed_at))
                .is_none_or(|(f, a)| f > a)
            || !crate::seconds_from_rfc3339(&attempt.failed_at)
                .zip(crate::seconds_from_rfc3339(&attempt.retry_after))
                .is_some_and(|(a, r)| r > a && r <= a + 300)
        {
            return Err(CommandLedgerError::Corrupt);
        }
        previous = attempt.failure_cursor;
    }
    if command
        .work
        .dispatch
        .as_ref()
        .is_some_and(|dispatch| dispatch.operation_sequence <= previous)
    {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(())
}
