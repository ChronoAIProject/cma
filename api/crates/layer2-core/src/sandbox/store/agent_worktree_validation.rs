use crate::sandbox::{AgentWorkStep, Worktree, WorktreeStatus};

use super::agent_work_transition_validation::PublicationKind;
use super::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};

pub(super) fn validate_worktree_delta(
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
