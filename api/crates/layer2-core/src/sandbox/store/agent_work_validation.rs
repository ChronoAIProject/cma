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
use super::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};

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
            || retired_sandbox_cleanup_is_absent(current, sandbox),
    )
}

/// Completed retirement has removed the entire runtime, not just an Agent process.
/// A stopped Agent may retain its interrupted turn as history; it cannot execute again.
pub fn retired_sandbox_cleanup_is_absent(agent: &Agent, sandbox: &Sandbox) -> bool {
    matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
        && matches!(
            agent.work.step,
            AgentWorkStep::EnsureProcessClosed
                | AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
        )
        && agent.work.cleanup_debt.is_empty()
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
        && agent.requested_stop_reason == Some(AgentStopReason::SandboxRetired)
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

fn validate_common(
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

fn immutable_agent_fields_changed(current: &Agent, next: &Agent) -> bool {
    current.id != next.id
        || current.worktree_id != next.worktree_id
        || current.workspace_id != next.workspace_id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.creation_key != next.creation_key
        || current.creation_fingerprint != next.creation_fingerprint
        || current.profile != next.profile
        || current.conversation.settings != next.conversation.settings
        || current.capabilities != next.capabilities
        || current.title != next.title
        || current.created_at != next.created_at
        || current.origin != next.origin
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
            if next.work.step != current.work.step
                || next.work.attempts != attempts
                || next.work.lease.is_some()
                || next.work.receipts != current.work.receipts
                || next.work.cleanup_debt != current.work.cleanup_debt
                || next.work.cleanup_not_before != current.work.cleanup_not_before
                || stamp(&next.work.ready_at)? <= stamp(&mutation.appended_at)?
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

fn validate_agent_delta(
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
) -> Result<(), SandboxStoreError> {
    let identity_changed = current.conversation.conversation_id
        != next.conversation.conversation_id
        || current.conversation.runtime_generation != next.conversation.runtime_generation;
    let opened = kind == PublicationKind::Advance
        && current.work.step == AgentWorkStep::OpenConversation
        && current.conversation.conversation_id.is_none()
        && current.conversation.runtime_generation.is_none()
        && next.conversation.conversation_id.is_some()
        && next.conversation.runtime_generation == Some(1);
    let conversation_unchanged = current.conversation.first_message
        == next.conversation.first_message
        && current.conversation.active_turn == next.conversation.active_turn
        && current.conversation.operation == next.conversation.operation
        && current.conversation.queued_messages == next.conversation.queued_messages
        && (!identity_changed || opened);
    if !conversation_unchanged {
        return Err(SandboxStoreError::Conflict);
    }
    let unchanged = current.status == next.status
        && current.failure == next.failure
        && current.ready_at == next.ready_at
        && current.stopped_at == next.stopped_at
        && current.requested_stop_reason == next.requested_stop_reason
        && current.stop_reason == next.stop_reason
        && current.deleted_at == next.deleted_at;
    let exact = match kind {
        PublicationKind::Intent
        | PublicationKind::StartupNotice
        | PublicationKind::Retry
        | PublicationKind::Advance => unchanged,
        PublicationKind::Ready => {
            current.failure == next.failure
                && next.ready_at.as_deref() == Some(&mutation.appended_at)
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::StartupFailure => {
            current.failure.is_none()
                && next.failure.is_some()
                && current.ready_at == next.ready_at
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::Stop => {
            current.failure == next.failure
                && current.ready_at == next.ready_at
                && next.stopped_at.as_deref() == Some(&mutation.appended_at)
                && next.requested_stop_reason.is_none()
                && next.stop_reason.as_ref() == current.requested_stop_reason.as_ref()
                && current.deleted_at == next.deleted_at
        }
        PublicationKind::Delete => {
            current.failure == next.failure
                && current.ready_at == next.ready_at
                && current.stopped_at == next.stopped_at
                && current.requested_stop_reason == next.requested_stop_reason
                && current.stop_reason == next.stop_reason
                && next.deleted_at.as_deref() == Some(&mutation.appended_at)
        }
    };
    if exact {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn validate_worktree_delta(
    current: &Worktree,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
    step: AgentWorkStep,
) -> Result<(), SandboxStoreError> {
    let expected_status = match (kind, step, current.status) {
        (PublicationKind::Ready, AgentWorkStep::PublishReady, WorktreeStatus::Creating) => {
            Some(WorktreeStatus::Ready)
        }
        (PublicationKind::StartupFailure | PublicationKind::Stop, _, WorktreeStatus::Creating) => {
            Some(WorktreeStatus::Failed)
        }
        (
            PublicationKind::Advance,
            AgentWorkStep::EnsureProcessClosed,
            WorktreeStatus::Ready | WorktreeStatus::Failed,
        ) => Some(WorktreeStatus::Removing),
        (PublicationKind::Advance, AgentWorkStep::RemoveWorktree, WorktreeStatus::Removing) => {
            Some(WorktreeStatus::Removed)
        }
        _ => None,
    };
    match (expected_status, mutation.worktree.as_ref()) {
        (Some(status), Some(versioned)) => validate_versioned_worktree(current, versioned, status),
        (None, None) => Ok(()),
        _ => Err(SandboxStoreError::Conflict),
    }
}

fn validate_versioned_worktree(
    current: &Worktree,
    versioned: &VersionedWorktree,
    expected_status: WorktreeStatus,
) -> Result<(), SandboxStoreError> {
    let next = &versioned.worktree;
    if versioned.expected_revision != current.revision
        || next.revision != current.revision.checked_add(1).unwrap_or(0)
        || next.status != expected_status
        || next.id != current.id
        || next.workspace_id != current.workspace_id
        || next.sandbox_id != current.sandbox_id
        || next.owner != current.owner
        || next.name != current.name
        || next.path != current.path
        || next.branch != current.branch
        || next.base_commit != current.base_commit
        || next.agent_id != current.agent_id
    {
        Err(SandboxStoreError::Conflict)
    } else {
        Ok(())
    }
}

pub(super) fn stamp(value: &str) -> Result<u64, SandboxStoreError> {
    seconds_from_rfc3339(value).ok_or(SandboxStoreError::Conflict)
}
