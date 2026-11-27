use super::agent_work_identity::{immutable_agent_fields_changed, validate_agent_delta};
use crate::sandbox::{
    Agent, AgentStatus, AgentStopReason, AgentWorkStep, Sandbox, SandboxStatus, SandboxWorkStep,
    Worktree, WorktreeStatus,
};
use crate::seconds_from_rfc3339;

use super::agent_work_evidence_validation::{
    startup_effect, validate_advance_history, validate_evidence, validate_histories,
};
use super::agent_work_transition_validation::{
    classify_publication, validate_position, PublicationKind,
};
use super::agent_worktree_validation::validate_worktree_delta;
use super::{agent_work_prepared_pair::paired_intent, AgentWorkMutation, SandboxStoreError};

/// Stores must read and write-fence both parents in the same transaction as this publication.
pub fn validate_agent_workspace_binding(
    current: &Agent,
    mutation: &AgentWorkMutation,
    sandbox: &Sandbox,
    workspace: &crate::sandbox::Workspace,
) -> Result<(), SandboxStoreError> {
    if current.work.step != AgentWorkStep::WaitForWorkspace
        || mutation.agent.work.step != AgentWorkStep::AddWorktree
    {
        return Ok(());
    }
    let exact_origin = matches!((&current.origin, &sandbox.origin),
        (crate::sandbox::AgentOrigin::Lifecycle { operation_id: agent_op },
        crate::sandbox::SandboxOrigin::Lifecycle { operation_id: parent_op }) if agent_op == parent_op);
    if !exact_origin
        || !current.has_no_runtime_effects()
        || current.owner != sandbox.owner
        || current.sandbox_id != sandbox.id
        || current.workspace_id != workspace.id
        || !crate::sandbox::lifecycle_bootstrap::bootstrap_workspace(sandbox, workspace)
        || sandbox.status != SandboxStatus::Active
        || !sandbox.credential.delivered
        || sandbox.lifecycle.operation.is_some()
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || workspace.status != crate::sandbox::WorkspaceStatus::Ready
        || workspace.resolved_commit.as_ref().is_none_or(|commit| {
            mutation
                .worktree
                .as_ref()
                .is_none_or(|next| &next.worktree.base_commit != commit)
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

/// Validate one fenced lifecycle publication against its complete durable snapshot.
pub fn validate_agent_work_mutation(
    current: &Agent,
    current_worktree: &Worktree,
    actual_last_seq: u64,
    mutation: &AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
    validate_mutation(current, current_worktree, actual_last_seq, mutation, false)
}

/// Validate cleanup whose process was already removed by the parent's durable teardown.
pub fn validate_agent_work_mutation_with_sandbox(
    current: &Agent,
    current_worktree: &Worktree,
    actual_last_seq: u64,
    mutation: &AgentWorkMutation,
    sandbox: &Sandbox,
) -> Result<(), SandboxStoreError> {
    validate_mutation(
        current,
        current_worktree,
        actual_last_seq,
        mutation,
        retiring_sandbox_process_is_absent(current, sandbox)
            || retired_sandbox_cleanup_is_absent(current, sandbox)
            || retired_failed_worktree_cleanup_is_absent(current, current_worktree, sandbox),
    )
}

/// Completed retirement has removed the entire runtime, not just an Agent process.
/// A stopped Agent may retain its interrupted turn as history; it cannot execute again.
pub fn retired_sandbox_cleanup_is_absent(agent: &Agent, sandbox: &Sandbox) -> bool {
    agent.work.cleanup_debt.is_empty() && retired_cleanup_parent_matches(agent, sandbox)
}

/// A released Sandbox home can settle the exact failed Worktree addition in its lineage.
pub fn retired_failed_worktree_cleanup_is_absent(
    agent: &Agent,
    worktree: &Worktree,
    sandbox: &Sandbox,
) -> bool {
    retired_cleanup_parent_matches(agent, sandbox)
        && sandbox
            .lifecycle
            .home
            .as_ref()
            .is_some_and(|home| home.released)
        && agent.status == AgentStatus::Failed
        && matches!(agent.work.cleanup_debt.as_slice(), [debt] if debt.step == AgentWorkStep::AddWorktree)
        && agent.work.receipts.is_empty()
        && agent.work.warm_conversation_id.is_none()
        && agent.ready_at.is_none()
        && agent.conversation.conversation_id.is_none()
        && agent.conversation.runtime_generation.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.operation.is_none()
        && worktree.id == agent.worktree_id
        && worktree.workspace_id == agent.workspace_id
        && worktree.sandbox_id == sandbox.id
        && worktree.owner == agent.owner
        && worktree.agent_id.as_ref() == Some(&agent.id)
        && matches!(
            (agent.work.step, worktree.status),
            (AgentWorkStep::EnsureProcessClosed, WorktreeStatus::Failed)
                | (
                    AgentWorkStep::RemoveHome | AgentWorkStep::RemoveWorktree,
                    WorktreeStatus::Removing
                )
        )
}

fn retired_cleanup_parent_matches(agent: &Agent, sandbox: &Sandbox) -> bool {
    matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
        && matches!(
            agent.work.step,
            AgentWorkStep::EnsureProcessClosed
                | AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
        )
        && agent.conversation.queued_messages.is_empty()
        && agent.sandbox_id == sandbox.id
        && agent.owner == sandbox.owner
        && sandbox.status == SandboxStatus::Retired
        && sandbox.retired_at.is_some()
        && sandbox.runtime.is_none()
        && sandbox.lifecycle.suspended_runtime.is_none()
        && sandbox.lifecycle.runtime_provision.is_none()
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && sandbox.validate().is_ok()
}

/// A retired-runtime close is not a runtime request receipt or a completed turn.
pub fn retiring_sandbox_process_is_absent(agent: &Agent, sandbox: &Sandbox) -> bool {
    agent.status == AgentStatus::Paused
        // Parent retirement supplies cleanup authority independently of who
        // originally requested this stop. Preserve that original attribution.
        && matches!(
            agent.requested_stop_reason,
            Some(AgentStopReason::SandboxRetired | AgentStopReason::Owner | AgentStopReason::BindingRetired)
        )
        && agent.work.step == AgentWorkStep::CloseProcess
        && agent.work.cleanup_debt.is_empty()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent.sandbox_id == sandbox.id
        && agent.owner == sandbox.owner
        && sandbox.status == SandboxStatus::Retiring
        && sandbox.work.step == SandboxWorkStep::StopAgents
        && sandbox.runtime.is_none()
        && sandbox.lifecycle.suspended_runtime.is_none()
        && sandbox.lifecycle.runtime_provision.is_none()
        && sandbox.lifecycle.operation.is_none()
        && sandbox.lifecycle.checkpoint_capture.is_none()
        && agent
            .conversation
            .runtime_generation
            .is_some_and(|generation| {
                // Runtime-loss parking advances the Agent fence without allocating a runtime.
                generation > 0
                    && sandbox.lifecycle.runtime_generation_high_water > 0
                    && sandbox
                        .lifecycle
                        .runtime_generation_high_water
                        .checked_add(1)
                        .is_some_and(|ceiling| generation <= ceiling)
            })
        && sandbox.validate().is_ok()
        && (sandbox
            .lifecycle
            .home
            .as_ref()
            .is_some_and(|home| !home.released && home.attachment().is_some())
            || sandbox
                .lifecycle
                .checkpoints
                .latest
                .as_ref()
                .is_some_and(|checkpoint| {
                    checkpoint.conversations.iter().any(|conversation| {
                        conversation.agent_id == agent.id
                            && Some(&conversation.conversation_id)
                                == agent.conversation.conversation_id.as_ref()
                    })
                }))
}

fn validate_mutation(
    current: &Agent,
    current_worktree: &Worktree,
    actual_last_seq: u64,
    mutation: &AgentWorkMutation,
    retired_process_absent: bool,
) -> Result<(), SandboxStoreError> {
    validate_common(current, current_worktree, actual_last_seq, mutation)?;
    if current.work.step == AgentWorkStep::RestartConversation
        && mutation.agent.status == AgentStatus::Ready
    {
        return super::agent_restart_ready::validate(current, current_worktree, mutation);
    }

    if let Some(result) =
        super::agent_log_capacity::validate_failure(current, current_worktree, mutation)
    {
        return result;
    }
    if current.work.step == AgentWorkStep::InstallSkill {
        return crate::sandbox::agent_live_skills::validate_install_work(
            current,
            current_worktree,
            mutation,
        );
    }
    if current.work.live_skills != mutation.agent.work.live_skills {
        return Err(SandboxStoreError::Conflict);
    }
    validate_position(current, current_worktree)?;
    validate_histories(current, &mutation.agent, &mutation.appended_at)?;
    let kind = classify_publication(current, &mutation.agent)?;
    validate_work_delta(
        current,
        &mutation.agent,
        mutation,
        kind,
        retired_process_absent,
    )?;
    validate_agent_delta(current, &mutation.agent, mutation, kind)?;
    validate_worktree_delta(current_worktree, mutation, kind, current.work.step)?;
    validate_evidence(current, current_worktree, mutation, kind)
}

pub(super) fn validate_common(
    current: &Agent,
    worktree: &Worktree,
    actual_last_seq: u64,
    mutation: &AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
    let next = &mutation.agent;
    let appended = stamp(&mutation.appended_at)?;
    let next_revision = mutation
        .expected_agent_revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    let log_count = u64::try_from(mutation.agent_logs.len())
        .map_err(|_| SandboxStoreError::SequenceConflict)?;
    let next_last_seq = current
        .conversation
        .last_seq
        .checked_add(log_count)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    if current.conversation.last_seq != actual_last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if current.revision != mutation.expected_agent_revision
        || next.revision != next_revision
        || mutation.holder.trim().is_empty()
        || mutation.fence == 0
        || !current.work.owns(&mutation.holder, mutation.fence)
        || current.work.step == AgentWorkStep::None
        || next.conversation.last_seq != next_last_seq
        || next.validate_conversation().is_err()
        || next.updated_at != mutation.appended_at
        || stamp(&current.updated_at)? > appended
        || stamp(&current.work.ready_at)? > appended
        || immutable_agent_fields_changed(current, next)
        || current
            .work
            .validate_resolved_skills(&current.profile.content.skills)
            .is_err()
        || next
            .work
            .validate_resolved_skills(&next.profile.content.skills)
            .is_err()
        || !worktree_matches_agent(worktree, current)
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn worktree_matches_agent(worktree: &Worktree, agent: &Agent) -> bool {
    worktree.revision > 0
        && worktree.id == agent.worktree_id
        && worktree.workspace_id == agent.workspace_id
        && worktree.sandbox_id == agent.sandbox_id
        && worktree.owner == agent.owner
        && worktree.agent_id.as_ref() == Some(&agent.id)
}

fn validate_work_delta(
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
    retired_process_absent: bool,
) -> Result<(), SandboxStoreError> {
    if next.work.next_fence != current.work.next_fence
        || next.work.warm_conversation_id != current.work.warm_conversation_id
    {
        return Err(SandboxStoreError::Conflict);
    }
    if kind == PublicationKind::Intent && current.work.step == AgentWorkStep::MaterializeHome {
        let warm_selection = current.work.warm_conversation_id.is_some()
            && current.work.resolved_skills.is_some()
            && current.work.resolved_skills == next.work.resolved_skills;
        if !warm_selection
            && (current.work.resolved_skills.is_some() || next.work.resolved_skills.is_none())
        {
            return Err(SandboxStoreError::Conflict);
        }
    } else if next.work.resolved_skills != current.work.resolved_skills {
        return Err(SandboxStoreError::Conflict);
    }
    if kind == PublicationKind::Advance
        && current.work.step == AgentWorkStep::MaterializeHome
        && current.work.resolved_skills.is_none()
    {
        return Err(SandboxStoreError::Conflict);
    }
    match kind {
        PublicationKind::Intent => validate_intent_delta(current, next),
        PublicationKind::StartupNotice => {
            if current.work == next.work
                && current.conversation.conversation_id.is_none()
                && current.conversation.runtime_generation.is_none()
                && current
                    .work
                    .cleanup_debt
                    .iter()
                    .any(|debt| debt.step == AgentWorkStep::OpenConversation)
                && current.work.lease.as_ref().is_some_and(|lease| {
                    stamp(&lease.expires_at).is_ok_and(|expires| {
                        stamp(&mutation.appended_at).is_ok_and(|at| at < expires)
                    })
                })
            {
                Ok(())
            } else {
                Err(SandboxStoreError::Conflict)
            }
        }
        PublicationKind::Retry => {
            let attempts = current
                .work
                .attempts
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?;
            // An effect-free parent wait already observed a bounded interval.
            // It may remain due for a readiness wake, while failure retries
            // still require a strictly later due time.
            let expected_parent_wait = current.work.step == AgentWorkStep::WaitForWorkspace
                && current.has_no_runtime_effects()
                && mutation.agent_logs.is_empty()
                && mutation.sandbox_event.is_none()
                && next.work.ready_at == mutation.appended_at;
            let retry_later = stamp(&next.work.ready_at)? > stamp(&mutation.appended_at)?;
            if next.work.step != current.work.step
                || next.work.attempts != attempts
                || next.work.lease.is_some()
                || next.work.receipts != current.work.receipts
                || next.work.cleanup_debt != current.work.cleanup_debt
                || next.work.cleanup_not_before != current.work.cleanup_not_before
                || !(retry_later || expected_parent_wait)
            {
                return Err(SandboxStoreError::Conflict);
            }
            Ok(())
        }
        _ => {
            let expected_ready = advance_ready_at(current, mutation)?;
            if next.work.attempts != 0
                || next.work.lease.is_some()
                || next.work.cleanup_not_before.is_some()
                || next.work.ready_at != expected_ready
            {
                return Err(SandboxStoreError::Conflict);
            }
            if kind == PublicationKind::StartupFailure {
                if next.work.receipts == current.work.receipts
                    && next.work.cleanup_debt == current.work.cleanup_debt
                {
                    Ok(())
                } else {
                    Err(SandboxStoreError::Conflict)
                }
            } else {
                validate_advance_history(current, next, mutation, retired_process_absent)
            }
        }
    }
}

fn validate_intent_delta(current: &Agent, next: &Agent) -> Result<(), SandboxStoreError> {
    if paired_intent(current, next) {
        return Ok(());
    }
    let debts = &next.work.cleanup_debt;
    let Some(debt) = debts.last() else {
        return Err(SandboxStoreError::Conflict);
    };
    if !startup_effect(current.work.step)
        || next.work.step != current.work.step
        || next.work.ready_at != current.work.ready_at
        || next.work.attempts != current.work.attempts
        || next.work.lease != current.work.lease
        || next.work.receipts != current.work.receipts
        || next.work.cleanup_not_before != current.work.cleanup_not_before
        || debts.len() != current.work.cleanup_debt.len() + 1
        || debts[..debts.len() - 1] != current.work.cleanup_debt
        || debt.step != current.work.step
        || current
            .work
            .cleanup_debt
            .iter()
            .any(|item| item.step == debt.step)
        || current
            .work
            .receipts
            .iter()
            .any(|item| item.step == debt.step)
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn advance_ready_at(
    current: &Agent,
    mutation: &AgentWorkMutation,
) -> Result<String, SandboxStoreError> {
    match (
        current.work.step,
        mutation.agent.work.step,
        current.work.cleanup_not_before.as_ref(),
    ) {
        (AgentWorkStep::FenceInteraction, AgentWorkStep::CloseProcess, Some(not_before)) => {
            if stamp(not_before)? > stamp(&mutation.appended_at)? {
                Ok(not_before.clone())
            } else {
                Ok(mutation.appended_at.clone())
            }
        }
        (_, _, Some(_)) => Err(SandboxStoreError::Conflict),
        _ => Ok(mutation.appended_at.clone()),
    }
}

pub(super) fn stamp(value: &str) -> Result<u64, SandboxStoreError> {
    seconds_from_rfc3339(value).ok_or(SandboxStoreError::Conflict)
}
