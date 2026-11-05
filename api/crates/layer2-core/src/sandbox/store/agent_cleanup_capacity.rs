//! Bound cleanup diagnostics without losing retry timing or effect authority.

use super::{AgentWorkMutation, SandboxStoreError, AGENT_LOG_CAPACITY_FAILURE};
use crate::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep, TypedFailure,
};
use crate::seconds_from_rfc3339;

const DETAIL: &str = "Cleanup publication reached the normal Agent history limit. The original cleanup request remains pending; no process or filesystem effect is claimed complete. Inspect runtime access and retained recovery evidence. Further retries retain timing and attempt counts without appending duplicate capacity diagnostics.";

/// Whether this proposal is a cleanup retry, not a completion or fresh work.
pub fn agent_cleanup_capacity_retry(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    current.work.step == mutation.agent.work.step
        && cleanup_step(current)
        && (mutation.agent_logs.is_empty() || marker(&mutation.agent_logs, current.work.step))
}

/// Validate the retained evidence authorizing a metadata-only retry. Stores
/// call this against the same complete snapshot as their normal publication
/// validation. One marker can be appended per step; its successors append none.
pub fn validate_agent_cleanup_capacity_log(
    current: &Agent,
    mutation: &AgentWorkMutation,
    last_log: Option<&AgentLogEntry>,
) -> Result<(), SandboxStoreError> {
    if !agent_cleanup_capacity_retry(current, mutation) {
        return Ok(());
    }
    let expected = plan_agent_cleanup_capacity_retry(
        current,
        last_log,
        &mutation.holder,
        mutation.fence,
        &mutation.appended_at,
        &mutation.agent.work.ready_at,
    )?;
    if mutation.expected_agent_revision == expected.expected_agent_revision
        && mutation.agent == expected.agent
        && mutation.agent_logs == expected.agent_logs
        && mutation.worktree.is_none()
        && mutation.sandbox_event.is_none()
    {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

/// Called only after an attempted cleanup publication returned a known-aborted
/// capacity error. This proposal does not retry any external effect itself.
pub fn plan_agent_cleanup_capacity_retry(
    current: &Agent,
    last_log: Option<&AgentLogEntry>,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    let stamp = seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    if !cleanup_step(current)
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
    let logs = if retained_marker(current, last_log) {
        Vec::new()
    } else {
        vec![AgentLogKind::WorkStepFailed {
            step: current.work.step,
            failure: TypedFailure {
                class: AGENT_LOG_CAPACITY_FAILURE.into(),
                detail: DETAIL.into(),
            },
        }]
    };
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
    agent.conversation.last_seq = current
        .conversation
        .last_seq
        .checked_add(logs.len() as u64)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    Ok(AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: holder.into(),
        fence,
        agent,
        worktree: None,
        appended_at: at.into(),
        sandbox_event: None,
        agent_logs: logs,
    })
}

fn retained_marker(agent: &Agent, entry: Option<&AgentLogEntry>) -> bool {
    entry.is_some_and(|entry| {
        entry.agent_id == agent.id
            && entry.seq == agent.conversation.last_seq
            && marker(std::slice::from_ref(&entry.kind), agent.work.step)
    })
}

fn marker(logs: &[AgentLogKind], step: AgentWorkStep) -> bool {
    matches!(logs, [AgentLogKind::WorkStepFailed { step: logged, failure }]
        if *logged == step && failure.class == AGENT_LOG_CAPACITY_FAILURE && failure.detail == DETAIL)
}

fn cleanup_step(agent: &Agent) -> bool {
    match agent.work.step {
        AgentWorkStep::FenceInteraction
        | AgentWorkStep::CloseProcess
        | AgentWorkStep::PublishStopped => {
            agent.status.is_live() && agent.requested_stop_reason.is_some()
        }
        AgentWorkStep::EnsureProcessClosed
        | AgentWorkStep::RemoveHome
        | AgentWorkStep::RemoveWorktree
        | AgentWorkStep::PublishDeleted => {
            matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
        }
        _ => false,
    }
}
