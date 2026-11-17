//! Bound pending-work diagnostics without losing retry timing or effect authority.

use super::{AgentWorkMutation, SandboxStoreError, AGENT_LOG_CAPACITY_FAILURE};
use crate::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep, TypedFailure,
};
use crate::seconds_from_rfc3339;

/// Normal retained-entry budget; stop and deletion reserves remain store-enforced.
pub const NORMAL_AGENT_LOG_ENTRIES: u64 = 10_000;

pub const AGENT_RUNTIME_LEDGER_HOLD: &str = "agent_runtime_ledger_hold";
pub const AGENT_RUNTIME_LEDGER_HOLD_DETAIL: &str = "Runtime mutation ledger capacity requires recovery. The original work remains pending; no runtime loss or resource removal is confirmed.";

pub const AGENT_RUNTIME_STARTUP_HOLD: &str = "agent_runtime_startup_hold";
pub const AGENT_RUNTIME_STARTUP_HOLD_DETAIL: &str = "Runtime startup is held after repeated startup failures. The original work remains pending; no process exit, resource removal or completed effect is confirmed.";

#[derive(Clone, Copy)]
enum MarkerKind {
    Capacity,
    LedgerHold,
    StartupHold,
}

impl MarkerKind {
    const ALL: [Self; 3] = [Self::Capacity, Self::LedgerHold, Self::StartupHold];

    fn diagnostic(self) -> (&'static str, &'static str) {
        match self {
            Self::Capacity => (AGENT_LOG_CAPACITY_FAILURE, DETAIL),
            Self::LedgerHold => (AGENT_RUNTIME_LEDGER_HOLD, AGENT_RUNTIME_LEDGER_HOLD_DETAIL),
            Self::StartupHold => (
                AGENT_RUNTIME_STARTUP_HOLD,
                AGENT_RUNTIME_STARTUP_HOLD_DETAIL,
            ),
        }
    }
}

const DETAIL: &str = "Cleanup publication reached the normal Agent history limit. The original cleanup request remains pending; no process or filesystem effect is claimed complete. Inspect runtime access and retained recovery evidence. Further retries retain timing and attempt counts without appending duplicate capacity diagnostics.";

/// Select cleanup/runtime hold markers and ordinary zero-entry retries. The store
/// separately proves the retained marker; this shape alone grants no authority.
pub fn agent_cleanup_capacity_retry(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    (super::agent_repeated_failure::retry_shape(current, mutation)
        && !matches!(
            current.work.step,
            AgentWorkStep::WaitForWorkspace | AgentWorkStep::InstallSkill
        ))
        || current.status == mutation.agent.status
            && current.work.step == mutation.agent.work.step
            && mutation.agent.work.lease.is_none()
            && (cleanup_step(current) || ledger_step(current))
            && ((mutation.agent_logs.is_empty()
                && current.work.step != AgentWorkStep::InstallSkill)
                || MarkerKind::ALL
                    .into_iter()
                    .any(|kind| marker(&mutation.agent_logs, current.work.step, kind)))
}

/// Validate the retained evidence authorizing a metadata-only retry. Stores
/// call this against the same complete snapshot as their normal publication
/// validation. One marker can be appended per step; its successors append none.
pub fn validate_agent_cleanup_capacity_log(
    current: &Agent,
    mutation: &AgentWorkMutation,
    last_log: Option<&AgentLogEntry>,
) -> Result<(), SandboxStoreError> {
    validate_agent_cleanup_capacity_log_with_retry(current, mutation, last_log, None)
}

/// Repeated ordinary failures carry their observed tail separately from append
/// entries so that a concurrent class or tail change cannot authorize a retry.
pub fn validate_agent_cleanup_capacity_log_with_retry(
    current: &Agent,
    mutation: &AgentWorkMutation,
    last_log: Option<&AgentLogEntry>,
    repeated_marker: Option<&AgentLogEntry>,
) -> Result<(), SandboxStoreError> {
    if let Some(marker) = repeated_marker {
        return super::validate_agent_repeated_failure(current, mutation, marker, last_log);
    }
    if !agent_cleanup_capacity_retry(current, mutation) {
        return Ok(());
    }
    let kind = MarkerKind::ALL
        .into_iter()
        .find(|kind| {
            marker(&mutation.agent_logs, current.work.step, *kind)
                || (mutation.agent_logs.is_empty() && retained_marker(current, last_log, *kind))
        })
        .unwrap_or(MarkerKind::Capacity);
    let expected = plan_retry(
        current,
        last_log,
        &mutation.holder,
        mutation.fence,
        &mutation.appended_at,
        &mutation.agent.work.ready_at,
        kind,
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
    plan_retry(
        current,
        last_log,
        holder,
        fence,
        at,
        ready_at,
        MarkerKind::Capacity,
    )
}

/// Cheap selection hint for an unchanged failed cleanup retry. This is never
/// authority to publish: the store must validate the complete log and claim.
pub fn agent_cleanup_capacity_retry_hint(current: &Agent) -> bool {
    cleanup_step(current) && current.conversation.last_seq > NORMAL_AGENT_LOG_ENTRIES
}

/// Select an already-marked failed cleanup retry without first attempting the
/// predictably rejected normal append. Unlike first-marker admission, this
/// requires an exact retained marker and returns only a metadata-only proposal.
/// The caller must have observed a failed work attempt; successful effects use
/// their ordinary completion path. The store revalidates this proposal, the
/// complete log, exact tail and unchanged revision/identity/lease in one snapshot.
pub fn plan_agent_marked_cleanup_capacity_retry(
    current: &Agent,
    last_log: Option<&AgentLogEntry>,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
) -> Result<Option<AgentWorkMutation>, SandboxStoreError> {
    if !agent_cleanup_capacity_retry_hint(current)
        || !retained_marker(current, last_log, MarkerKind::Capacity)
    {
        return Ok(None);
    }
    plan_retry(
        current,
        last_log,
        holder,
        fence,
        at,
        ready_at,
        MarkerKind::Capacity,
    )
    .map(Some)
}

/// Keep a reachable runtime's refused work pending, including at the normal
/// Agent log limit. An exact retained marker avoids duplicate diagnostics.
/// Stores validate this complete proposal and marker in their fenced snapshot.
pub fn plan_agent_runtime_ledger_hold_retry(
    current: &Agent,
    last_log: Option<&AgentLogEntry>,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    plan_retry(
        current,
        last_log,
        holder,
        fence,
        at,
        ready_at,
        MarkerKind::LedgerHold,
    )
}

/// Keep repeated startup failures pending using one exact retained tail marker.
/// A different hold class requires a new marker under the store's usual fence.
pub fn plan_agent_runtime_startup_hold_retry(
    current: &Agent,
    last_log: Option<&AgentLogEntry>,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    plan_retry(
        current,
        last_log,
        holder,
        fence,
        at,
        ready_at,
        MarkerKind::StartupHold,
    )
}

fn plan_retry(
    current: &Agent,
    last_log: Option<&AgentLogEntry>,
    holder: &str,
    fence: u64,
    at: &str,
    ready_at: &str,
    kind: MarkerKind,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    let stamp = seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    if !(match kind {
        MarkerKind::Capacity => cleanup_step(current),
        MarkerKind::LedgerHold | MarkerKind::StartupHold => ledger_step(current),
    }) || holder.trim().is_empty()
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
    let logs = if retained_marker(current, last_log, kind) {
        Vec::new()
    } else {
        vec![AgentLogKind::WorkStepFailed {
            step: current.work.step,
            failure: TypedFailure {
                class: kind.diagnostic().0.into(),
                detail: kind.diagnostic().1.into(),
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

fn retained_marker(agent: &Agent, entry: Option<&AgentLogEntry>, kind: MarkerKind) -> bool {
    entry.is_some_and(|entry| {
        entry.agent_id == agent.id
            && entry.seq == agent.conversation.last_seq
            && marker(std::slice::from_ref(&entry.kind), agent.work.step, kind)
    })
}

fn marker(logs: &[AgentLogKind], step: AgentWorkStep, kind: MarkerKind) -> bool {
    let (class, detail) = kind.diagnostic();
    matches!(logs, [AgentLogKind::WorkStepFailed { step: logged, failure }]
        if *logged == step && failure.class == class && failure.detail == detail)
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

fn ledger_step(agent: &Agent) -> bool {
    cleanup_step(agent)
        || agent.status.is_live()
            && matches!(
                agent.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::DispatchFirstMessage
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::InstallSkill
            )
}
