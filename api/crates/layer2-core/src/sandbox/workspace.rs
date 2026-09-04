use serde::{Deserialize, Serialize};

use crate::ids::{SandboxId, SubjectId, WorkspaceId};
use crate::profiles::{validate_workspace_name, WorkspaceProfileSnapshot};

use super::{Sandbox, SandboxModelError, TypedFailure, WorkspaceStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceSource {
    GitRepository {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_commit: Option<String>,
    },
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub profile: WorkspaceProfileSnapshot,
    pub name: String,
    pub path: String,
    pub source: WorkspaceSource,
    pub status: WorkspaceStatus,
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
    pub fn validate_prerequisites(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        if sandbox.id != self.sandbox_id || sandbox.owner != self.owner {
            return Err(SandboxModelError::Prerequisite {
                resource: "workspace",
                detail: "sandbox lineage does not match",
            });
        }
        if !sandbox.status.accepts_work() {
            return Err(SandboxModelError::Prerequisite {
                resource: "workspace",
                detail: "sandbox is not active",
            });
        }
        validate_workspace_name(&self.name)
            .map_err(|_| SandboxModelError::invalid_value("workspace.name"))?;
        if self.path != format!("/home/cma/{}", self.name) || self.revision == 0 {
            return Err(SandboxModelError::invalid_value("workspace.path"));
        }
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
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("workspace.revision"))?;
        self.status = status;
        self.failed = failure;
        match next {
            WorkspaceStatus::Ready => self.ready_at = Some(at.clone()),
            WorkspaceStatus::Removed => self.removed_at = Some(at.clone()),
            _ => {}
        }
        self.revision = revision;
        Ok(())
    }
}
