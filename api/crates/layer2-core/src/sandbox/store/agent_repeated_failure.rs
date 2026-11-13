//! A retained failure is evidence for retry metadata, never for a completed effect.

use super::{
    AgentWorkMutation, SandboxStoreError, AGENT_LOG_CAPACITY_FAILURE, AGENT_RUNTIME_LEDGER_HOLD,
};
use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, AgentWorkStep};
use crate::seconds_from_rfc3339;

/// Transient publication evidence. No new field is stored on the Agent or log.
#[derive(Debug, Clone)]
pub struct AgentRepeatedFailure {
    pub mutation: AgentWorkMutation,
    pub marker: AgentLogEntry,
}

/// Select only a matching ordinary failure. Capacity and ledger markers retain
/// their own priority, exact-detail contracts and reserve rules.
pub fn plan_agent_repeated_failure(
    current: &Agent,
    last: Option<&AgentLogEntry>,
    failure_class: &str,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
) -> Result<Option<AgentRepeatedFailure>, SandboxStoreError> {
    let Some(marker) = last.filter(|entry| {
        entry.agent_id == current.id
            && entry.seq == current.conversation.last_seq
            && matches!(&entry.kind, AgentLogKind::WorkStepFailed { step, failure }
                if *step == current.work.step && failure.class == failure_class
                    && !failure.class.is_empty()
                    && !matches!(failure.class.as_str(), AGENT_LOG_CAPACITY_FAILURE | AGENT_RUNTIME_LEDGER_HOLD))
    }) else {
        return Ok(None);
    };
    let stamp = seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    if current.work.step == AgentWorkStep::None
        || holder.trim().is_empty()
        || fence == 0
        || !current.work.owns(holder, fence)
        || current.work.lease.as_ref().is_none_or(|lease| {
            seconds_from_rfc3339(&lease.expires_at).is_none_or(|expires| stamp >= expires)
        })
        || seconds_from_rfc3339(ready_at).is_none_or(|ready| ready <= stamp)
        || seconds_from_rfc3339(&current.updated_at).is_none_or(|updated| updated > stamp)
        || seconds_from_rfc3339(&current.work.ready_at).is_none_or(|ready| ready > stamp)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut agent = current.clone();
    agent
        .work
        .retry(holder, fence, ready_at.into())
        .map_err(|_| SandboxStoreError::Conflict)?;
    agent.updated_at = at.into();
    agent.revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    Ok(Some(AgentRepeatedFailure {
        marker: marker.clone(),
        mutation: AgentWorkMutation {
            expected_agent_revision: current.revision,
            holder: holder.into(),
            fence,
            agent,
            worktree: None,
            appended_at: at.into(),
            sandbox_event: None,
            agent_logs: Vec::new(),
        },
    }))
}

/// Recheck both the observed marker and the entire proposed mutation against
/// the store's own snapshot, including the original lease and revision.
pub fn validate_agent_repeated_failure(
    current: &Agent,
    mutation: &AgentWorkMutation,
    observed: &AgentLogEntry,
    last: Option<&AgentLogEntry>,
) -> Result<(), SandboxStoreError> {
    if last != Some(observed) {
        return Err(SandboxStoreError::Conflict);
    }
    let AgentLogKind::WorkStepFailed { failure, .. } = &observed.kind else {
        return Err(SandboxStoreError::Conflict);
    };
    let expected = plan_agent_repeated_failure(
        current,
        last,
        &failure.class,
        &mutation.holder,
        mutation.fence,
        &mutation.appended_at,
        &mutation.agent.work.ready_at,
    )?
    .ok_or(SandboxStoreError::Conflict)?
    .mutation;
    if mutation.expected_agent_revision != expected.expected_agent_revision
        || mutation.agent != expected.agent
        || !mutation.agent_logs.is_empty()
        || mutation.worktree.is_some()
        || mutation.sandbox_event.is_some()
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

/// Selection only. Stores must validate explicit retained-marker evidence.
pub(super) fn retry_shape(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    current.work.step != AgentWorkStep::None
        && current.status == mutation.agent.status
        && current.work.step == mutation.agent.work.step
        && mutation.agent.work.lease.is_none()
        && mutation.agent_logs.is_empty()
}
