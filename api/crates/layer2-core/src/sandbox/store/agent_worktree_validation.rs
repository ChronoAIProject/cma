use crate::sandbox::{AgentWorkStep, Worktree, WorktreeStatus};

use super::agent_work_transition_validation::PublicationKind;
use super::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};

pub(super) fn validate_worktree_delta(
    current: &Worktree,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
    step: AgentWorkStep,
) -> Result<(), SandboxStoreError> {
    if kind == PublicationKind::Advance && step == AgentWorkStep::WaitForWorkspace {
        let versioned = mutation
            .worktree
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let commit = &versioned.worktree.base_commit;
        if !current.base_commit.is_empty()
            || !matches!(commit.len(), 40 | 64)
            || !commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut original = current.clone();
        original.base_commit.clone_from(commit);
        return validate_versioned_worktree(&original, versioned, WorktreeStatus::Creating);
    }
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
            WorktreeStatus::Creating | WorktreeStatus::Ready | WorktreeStatus::Failed,
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
