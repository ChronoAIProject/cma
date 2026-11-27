use crate::sandbox::{Agent, AgentStatus, AgentWorkStep, Worktree, WorktreeStatus};
use crate::seconds_from_rfc3339;

use super::agent_work_prepared_pair::{paired_completion, paired_intent};
use super::SandboxStoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublicationKind {
    Intent,
    StartupNotice,
    Retry,
    Advance,
    Ready,
    StartupFailure,
    Stop,
    Delete,
}

pub(super) fn validate_position(
    agent: &Agent,
    worktree: &Worktree,
    retired_process_absent: bool,
) -> Result<(), SandboxStoreError> {
    let live_shape = !agent.status.is_live()
        || (agent.failure.is_none()
            && agent.stopped_at.is_none()
            && agent.stop_reason.is_none()
            && agent.deleted_at.is_none());
    let terminal_shape = match agent.status {
        AgentStatus::Stopped => {
            agent.failure.is_none()
                && agent.stopped_at.is_some()
                && agent.stop_reason.is_some()
                && agent.requested_stop_reason.is_none()
                && agent.deleted_at.is_none()
        }
        AgentStatus::Failed => {
            agent.failure.is_some()
                && agent.stopped_at.is_none()
                && agent.stop_reason.is_none()
                && agent.requested_stop_reason.is_none()
                && agent.deleted_at.is_none()
        }
        _ => true,
    };
    let positioned = match agent.work.step {
        AgentWorkStep::InstallSkill => false,
        AgentWorkStep::WaitForWorkspace => {
            agent.status == AgentStatus::Starting
                && agent.requested_stop_reason.is_none()
                && agent.has_no_runtime_effects()
                && worktree.status == WorktreeStatus::Creating
                && worktree.base_commit.is_empty()
        }
        AgentWorkStep::AddWorktree
        | AgentWorkStep::MaterializeHome
        | AgentWorkStep::OpenConversation
        | AgentWorkStep::PublishReady => {
            agent.status == AgentStatus::Starting
                && agent.ready_at.is_none()
                && agent.requested_stop_reason.is_none()
                && worktree.status == WorktreeStatus::Creating
        }
        AgentWorkStep::PublishConnection => {
            matches!(
                agent.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            ) && agent
                .ready_at
                .as_deref()
                .and_then(seconds_from_rfc3339)
                .is_some()
                && agent.requested_stop_reason.is_none()
                && worktree.status == WorktreeStatus::Ready
        }
        AgentWorkStep::DispatchFirstMessage => {
            matches!(
                agent.status,
                AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused
            ) && agent
                .ready_at
                .as_deref()
                .and_then(seconds_from_rfc3339)
                .is_some()
                && agent.requested_stop_reason.is_none()
                && worktree.status == WorktreeStatus::Ready
        }
        AgentWorkStep::FenceInteraction
        | AgentWorkStep::CloseProcess
        | AgentWorkStep::PublishStopped => {
            agent.status.is_live()
                && agent.requested_stop_reason.is_some()
                && matches!(
                    worktree.status,
                    WorktreeStatus::Creating | WorktreeStatus::Ready | WorktreeStatus::Failed
                )
        }
        AgentWorkStep::EnsureProcessClosed => {
            matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
                && matches!(
                    worktree.status,
                    WorktreeStatus::Ready | WorktreeStatus::Failed
                )
                || (agent.status == AgentStatus::Stopped
                    && worktree.status == WorktreeStatus::Creating
                    && retired_process_absent)
        }
        AgentWorkStep::RemoveHome | AgentWorkStep::RemoveWorktree => {
            matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
                && worktree.status == WorktreeStatus::Removing
        }
        AgentWorkStep::PublishDeleted => {
            matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
                && worktree.status == WorktreeStatus::Removed
        }
        AgentWorkStep::RestartConversation => {
            agent.status == AgentStatus::Starting
                && agent.ready_at.is_some()
                && agent.requested_stop_reason.is_none()
                && worktree.status == WorktreeStatus::Ready
        }
        AgentWorkStep::RestartTeardown => {
            agent.status == AgentStatus::Starting
                && agent.ready_at.is_some()
                && agent.requested_stop_reason.is_some()
                && worktree.status == WorktreeStatus::Ready
        }
        AgentWorkStep::None => false,
    };
    if live_shape && terminal_shape && positioned {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

pub(super) fn classify_publication(
    current: &Agent,
    next: &Agent,
) -> Result<PublicationKind, SandboxStoreError> {
    let from = current.work.step;
    let to = next.work.step;
    if from == to {
        if from == AgentWorkStep::OpenConversation && current.work == next.work {
            return Ok(PublicationKind::StartupNotice);
        }
        return Ok(
            if next.work.cleanup_debt.len() == current.work.cleanup_debt.len() + 1
                || paired_intent(current, next)
            {
                PublicationKind::Intent
            } else {
                PublicationKind::Retry
            },
        );
    }
    if from == AgentWorkStep::PublishReady
        && to == AgentWorkStep::PublishConnection
        && current.status == AgentStatus::Starting
        && next.status == AgentStatus::Ready
    {
        return Ok(PublicationKind::Ready);
    }
    if startup_step(from)
        && to == AgentWorkStep::None
        && current.status == AgentStatus::Starting
        && next.status == AgentStatus::Failed
    {
        return Ok(PublicationKind::StartupFailure);
    }
    if from == AgentWorkStep::PublishStopped
        && to == AgentWorkStep::None
        && current.status.is_live()
        && next.status == AgentStatus::Stopped
    {
        return Ok(PublicationKind::Stop);
    }
    if from == AgentWorkStep::PublishDeleted
        && to == AgentWorkStep::None
        && matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
        && next.status == AgentStatus::Deleted
    {
        return Ok(PublicationKind::Delete);
    }
    if current.status == next.status
        && (legal_advance(from, to) || paired_completion(current, next))
    {
        Ok(PublicationKind::Advance)
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn startup_step(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::WaitForWorkspace
            | AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
            | AgentWorkStep::PublishReady
    )
}

fn legal_advance(from: AgentWorkStep, to: AgentWorkStep) -> bool {
    matches!(
        (from, to),
        (AgentWorkStep::WaitForWorkspace, AgentWorkStep::AddWorktree)
            | (AgentWorkStep::AddWorktree, AgentWorkStep::MaterializeHome)
            | (
                AgentWorkStep::MaterializeHome,
                AgentWorkStep::OpenConversation
            )
            | (AgentWorkStep::OpenConversation, AgentWorkStep::PublishReady)
            | (
                AgentWorkStep::PublishConnection,
                AgentWorkStep::DispatchFirstMessage
                    | AgentWorkStep::InstallSkill
                    | AgentWorkStep::None
            )
            | (AgentWorkStep::DispatchFirstMessage, AgentWorkStep::None)
            | (AgentWorkStep::FenceInteraction, AgentWorkStep::CloseProcess)
            | (AgentWorkStep::CloseProcess, AgentWorkStep::PublishStopped)
            | (
                AgentWorkStep::EnsureProcessClosed,
                AgentWorkStep::RemoveHome
            )
            | (AgentWorkStep::RemoveHome, AgentWorkStep::RemoveWorktree)
            | (AgentWorkStep::RemoveWorktree, AgentWorkStep::PublishDeleted)
    )
}
