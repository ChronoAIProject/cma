//! A bounded failure proposal after a known-aborted lifecycle log append.
//! The caller must still commit through the ordinary fenced publication store.

use super::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};
use crate::sandbox::{
    Agent, AgentLogKind, AgentSkillInstallStatus, AgentStatus, AgentWorkStep, SandboxEventKind,
    TypedFailure, Worktree, WorktreeStatus,
};
use crate::seconds_from_rfc3339;

pub const AGENT_LOG_CAPACITY_FAILURE: &str = "agent_log_capacity_exceeded";

/// Plan a terminal lifecycle outcome without asserting that the native process
/// stopped or that an admitted input was never delivered. This deliberately
/// preserves conversation recovery facts; any proved first-input settlement is
/// performed by the shared first-message contract at the publication boundary.
pub fn plan_agent_log_capacity_failure(
    current: &Agent,
    worktree: &Worktree,
    holder: &str,
    fence: u64,
    at: &str,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    let stamp = seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    if !normal_work(current)
        || current.failure.is_some()
        || current.requested_stop_reason.is_some()
        || current.stopped_at.is_some()
        || current.deleted_at.is_some()
        || current.stop_reason.is_some()
        || current.work.cleanup_not_before.is_some()
        || holder.trim().is_empty()
        || fence == 0
        || !current.work.owns(holder, fence)
        || current.work.lease.as_ref().is_none_or(|lease| {
            seconds_from_rfc3339(&lease.expires_at).is_none_or(|expires| stamp >= expires)
        })
        || seconds_from_rfc3339(&current.updated_at).is_none_or(|updated| updated > stamp)
        || seconds_from_rfc3339(&current.work.ready_at).is_none_or(|ready| ready > stamp)
        || worktree.id != current.worktree_id
        || worktree.workspace_id != current.workspace_id
        || worktree.sandbox_id != current.sandbox_id
        || worktree.owner != current.owner
        || worktree.agent_id.as_ref() != Some(&current.id)
        || !matches!(
            worktree.status,
            WorktreeStatus::Creating | WorktreeStatus::Ready
        )
        || current.validate_conversation().is_err()
    {
        return Err(SandboxStoreError::Conflict);
    }
    let failure = TypedFailure {
        class: AGENT_LOG_CAPACITY_FAILURE.into(),
        detail: "Durable Agent history cannot accept this lifecycle event. Retained history and runtime identity are preserved. This Agent cannot resume under the current limits; inspect capacity before explicitly replacing or recovering it.".into(),
    };
    let mut next = current.clone();
    if let Some(first) = next.conversation.first_message.as_mut() {
        // Reuse #1318's reader-compatible proof of never-dispatched input.
        // Attempted or ambiguous input must retain every original delivery fact.
        let mut settled = first.clone();
        if settled.fail_startup(at.into()).is_ok() {
            *first = settled;
        }
    }
    next.work
        .advance(holder, fence, AgentWorkStep::None, at.into())
        .map_err(|_| SandboxStoreError::Conflict)?;
    if let Some(skill) = next
        .work
        .live_skills
        .last_mut()
        .filter(|skill| skill.status == AgentSkillInstallStatus::Pending)
    {
        skill.status = AgentSkillInstallStatus::Failed;
        skill.error = Some(AGENT_LOG_CAPACITY_FAILURE.into());
    }
    // Capacity failure is an explicit domain transition, not a generic runtime
    // failure permission for every live status. Its validator compares this
    // complete proposal, preserving the narrower ordinary transition graph.
    next.status = AgentStatus::Failed;
    next.failure = Some(failure.clone());
    next.updated_at = at.into();
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.conversation.last_seq = current
        .conversation
        .last_seq
        .checked_add(1)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let worktree = if worktree.status == WorktreeStatus::Creating {
        let expected_revision = worktree.revision;
        let mut failed = worktree.clone();
        failed
            .transition(WorktreeStatus::Failed)
            .map_err(|_| SandboxStoreError::Conflict)?;
        Some(VersionedWorktree {
            expected_revision,
            worktree: failed,
        })
    } else {
        None
    };
    Ok(AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: holder.into(),
        fence,
        agent: next,
        worktree,
        appended_at: at.into(),
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: current.id.clone(),
            from: current.status,
            to: AgentStatus::Failed,
        }),
        agent_logs: vec![AgentLogKind::WorkStepFailed {
            step: current.work.step,
            failure,
        }],
    })
}

pub(super) fn validate_failure(
    current: &Agent,
    worktree: &Worktree,
    mutation: &AgentWorkMutation,
) -> Option<Result<(), SandboxStoreError>> {
    if !current.status.is_live()
        || mutation.agent.status != AgentStatus::Failed
        || mutation
            .agent
            .failure
            .as_ref()
            .is_none_or(|failure| failure.class != AGENT_LOG_CAPACITY_FAILURE)
    {
        return None;
    }
    Some((|| {
        if current.work.step == AgentWorkStep::InstallSkill {
            if worktree.status != WorktreeStatus::Ready
                || current
                    .ready_at
                    .as_deref()
                    .and_then(seconds_from_rfc3339)
                    .is_none()
                || current
                    .work
                    .live_skills
                    .last()
                    .is_none_or(|skill| skill.status != AgentSkillInstallStatus::Pending)
            {
                return Err(SandboxStoreError::Conflict);
            }
        } else {
            super::agent_work_transition_validation::validate_position(current, worktree, false)?;
        }
        super::agent_work_evidence_validation::validate_histories(
            current,
            &mutation.agent,
            &mutation.appended_at,
        )?;
        let expected = plan_agent_log_capacity_failure(
            current,
            worktree,
            &mutation.holder,
            mutation.fence,
            &mutation.appended_at,
        )?;
        if mutation.agent != expected.agent
            || mutation.agent_logs != expected.agent_logs
            || mutation.sandbox_event != expected.sandbox_event
            || mutation
                .worktree
                .as_ref()
                .map(|v| (v.expected_revision, &v.worktree))
                != expected
                    .worktree
                    .as_ref()
                    .map(|v| (v.expected_revision, &v.worktree))
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    })())
}

fn normal_work(agent: &Agent) -> bool {
    match agent.work.step {
        AgentWorkStep::WaitForWorkspace
        | AgentWorkStep::AddWorktree
        | AgentWorkStep::MaterializeHome
        | AgentWorkStep::OpenConversation
        | AgentWorkStep::PublishReady => agent.status == AgentStatus::Starting,
        AgentWorkStep::InstallSkill => {
            agent.status == AgentStatus::Ready && agent.conversation.active_turn.is_none()
        }
        AgentWorkStep::PublishConnection => matches!(
            agent.status,
            AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
        ),
        AgentWorkStep::DispatchFirstMessage => matches!(
            agent.status,
            AgentStatus::Ready
                | AgentStatus::Working
                | AgentStatus::WaitingApproval
                | AgentStatus::Paused
        ),
        _ => false,
    }
}
