//! Detail projections carry only the identity needed for counts and ancestry.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxAgentLineage {
    pub id: AgentId,
    pub sandbox_id: SandboxId,
    pub workspace_id: WorkspaceId,
    pub worktree_id: WorktreeId,
    pub owner: SubjectId,
    pub revision: u64,
}

/// Full records remain the store contract for lifecycle and recovery consumers.
/// Only the detail port permits the explicitly selected lineage projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxDetailRecord {
    Complete(SandboxHierarchyRecord),
    Projected(SandboxHierarchyRecord<SandboxAgentLineage>),
}
impl SandboxDetailRecord {
    pub fn sandbox(&self) -> &Sandbox {
        match self {
            Self::Complete(record) => &record.sandbox,
            Self::Projected(record) => &record.sandbox,
        }
    }
}

pub(super) struct Lineage<'a> {
    pub id: &'a AgentId,
    pub sandbox_id: &'a SandboxId,
    pub workspace_id: &'a WorkspaceId,
    pub worktree_id: &'a WorktreeId,
    pub owner: &'a SubjectId,
    pub revision: u64,
}

pub(super) trait DetailAgent {
    fn lineage(&self) -> Lineage<'_>;
    fn validate_payload(&self) -> Result<(), HierarchyReadError>;
}
impl DetailAgent for Agent {
    fn lineage(&self) -> Lineage<'_> {
        Lineage {
            id: &self.id,
            sandbox_id: &self.sandbox_id,
            workspace_id: &self.workspace_id,
            worktree_id: &self.worktree_id,
            owner: &self.owner,
            revision: self.revision,
        }
    }
    fn validate_payload(&self) -> Result<(), HierarchyReadError> {
        self.validate_conversation()
            .map_err(|_| corrupt("invalid Agent conversation"))
    }
}
impl DetailAgent for SandboxAgentLineage {
    fn lineage(&self) -> Lineage<'_> {
        Lineage {
            id: &self.id,
            sandbox_id: &self.sandbox_id,
            workspace_id: &self.workspace_id,
            worktree_id: &self.worktree_id,
            owner: &self.owner,
            revision: self.revision,
        }
    }
    fn validate_payload(&self) -> Result<(), HierarchyReadError> {
        // Conversation fields are absent by contract. Their readers retain full
        // decoding and validation; detail still validates every lineage edge.
        Ok(())
    }
}
