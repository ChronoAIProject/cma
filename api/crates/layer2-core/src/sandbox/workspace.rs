use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, SubjectId, WorkspaceId};
use crate::profiles::{validate_workspace_name, WorkspaceProfileSnapshot};

use super::{
    Sandbox, SandboxModelError, TypedFailure, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep,
};

/// The Sandbox home that every Workspace, Worktree and Agent file path is under.
pub(crate) const SANDBOX_HOME: &str = "/home/cma/";

/// Why this workspace was admitted. Profile entries use the stable index as
/// their replay identity; a sandbox worker fence is deliberately excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceOrigin {
    Manual,
    SandboxProfile { index: u16 },
    Trigger { launch_id: String },
    Lifecycle { operation_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceSource {
    RepositorySnapshot {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_commit: Option<String>,
    },
    PublicRepositorySnapshot {
        url: String,
        commit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_commit: Option<String>,
    },
    GitRepository {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub origin: WorkspaceOrigin,
    pub profile: WorkspaceProfileSnapshot,
    pub name: String,
    pub path: String,
    pub source: WorkspaceSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    pub status: WorkspaceStatus,
    pub work: WorkspaceWorkState,
    pub next_worktree_number: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
    pub revision: u64,
}

impl Workspace {
    /// Bind profile-origin admission to the exact slot in the parent snapshot.
    pub fn origin_matches_sandbox_profile(&self, sandbox: &Sandbox) -> bool {
        let WorkspaceOrigin::SandboxProfile { index } = &self.origin else {
            return true;
        };
        sandbox
            .profile
            .content
            .workspace_profiles
            .get(usize::from(*index))
            .is_some_and(|reference| {
                reference.id == self.profile.id && reference.revision == self.profile.revision
            })
    }

    pub fn validate_origin(&self) -> Result<(), SandboxModelError> {
        if let Some(execution) = self.work.execution {
            execution.validate()?;
        }
        if let WorkspaceOrigin::Trigger { launch_id }
        | WorkspaceOrigin::Lifecycle {
            operation_id: launch_id,
        } = &self.origin
        {
            if launch_id.trim().is_empty()
                || launch_id.len() > 128
                || launch_id.chars().any(char::is_control)
            {
                return Err(SandboxModelError::invalid_value("workspace.origin"));
            }
        }
        Ok(())
    }

    pub fn validate_prerequisites(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.validate_for_admission(sandbox, super::SandboxStatus::Active)
    }

    pub fn validate_for_admission(
        &self,
        sandbox: &Sandbox,
        allowed_status: super::SandboxStatus,
    ) -> Result<(), SandboxModelError> {
        if sandbox.id != self.sandbox_id || sandbox.owner != self.owner {
            return Err(SandboxModelError::Prerequisite {
                resource: "workspace",
                detail: "sandbox lineage does not match",
            });
        }
        if sandbox.status != allowed_status {
            return Err(SandboxModelError::Prerequisite {
                resource: "workspace",
                detail: "sandbox is not active",
            });
        }
        validate_workspace_name(&self.name)
            .map_err(|_| SandboxModelError::invalid_value("workspace.name"))?;
        self.validate_origin()?;
        if self.path != format!("{SANDBOX_HOME}{}", self.name) || self.revision == 0 {
            return Err(SandboxModelError::invalid_value("workspace.path"));
        }
        if self.work.next_fence == 0 || self.next_worktree_number == 0 {
            return Err(SandboxModelError::invalid_value("workspace.work"));
        }
        Ok(())
    }

    pub fn operation_id(&self, step: WorkspaceWorkStep) -> String {
        format!("workspace:{}:{}:v1", self.id, step.operation_name())
    }

    pub fn record_resolved_commit(&mut self, commit: String) -> Result<(), SandboxModelError> {
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(SandboxModelError::invalid_value(
                "workspace.resolved_commit",
            ));
        }
        self.resolved_commit = Some(commit.to_ascii_lowercase());
        Ok(())
    }

    pub fn transition(
        &mut self,
        next: WorkspaceStatus,
        at: String,
        failure: Option<TypedFailure>,
    ) -> Result<(), SandboxModelError> {
        if (next == WorkspaceStatus::Failed) != failure.is_some() {
            return Err(SandboxModelError::invalid_value("workspace.failed"));
        }
        let status = self.status.transition(next)?;
        if next == WorkspaceStatus::Ready && self.resolved_commit.is_none() {
            return Err(SandboxModelError::invalid_value(
                "workspace.resolved_commit",
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("workspace.revision"))?;
        self.status = status;
        self.failed = failure;
        match next {
            WorkspaceStatus::Ready => {
                self.ready_at = Some(at.clone());
            }
            WorkspaceStatus::Removed => self.removed_at = Some(at.clone()),
            _ => {}
        }
        self.revision = revision;
        Ok(())
    }
}
