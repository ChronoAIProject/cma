use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceAccessSnapshot, ResourceOwner, ResourceTarget};
use crate::sandbox::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};

use super::super::BridgeAgentTarget;
use super::BridgeInvokeError;

/// Stable retained creation identity. Runtime incarnations and ordinary resource
/// revisions are intentionally excluded; live proof separately fences them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeTarget {
    pub ids: BridgeAgentTarget,
    pub sandbox_created_at: String,
    pub workspace_created_at: String,
    pub agent_created_at: String,
    pub agent_creation_fingerprint: String,
    pub worktree_identity_digest: String,
}

impl BridgeInvokeTarget {
    pub fn capture(snapshot: &ResourceAccessSnapshot) -> Result<Self, BridgeInvokeError> {
        let lineage = &snapshot.lineage;
        let agent = lineage
            .agent
            .as_ref()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        snapshot
            .scope(&ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            })
            .map_err(|_| BridgeInvokeError::TargetMismatch)?;
        let workspace = lineage
            .workspace
            .as_ref()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let worktree = lineage
            .worktree
            .as_ref()
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let result = Self {
            ids: BridgeAgentTarget {
                sandbox_id: lineage.sandbox.id.clone(),
                workspace_id: workspace.id.clone(),
                worktree_id: worktree.id.clone(),
                agent_id: agent.id.clone(),
            },
            sandbox_created_at: lineage.sandbox.created_at.clone(),
            workspace_created_at: workspace.created_at.clone(),
            agent_created_at: agent.created_at.clone(),
            agent_creation_fingerprint: agent.creation_fingerprint.clone(),
            // Worktrees have no creation timestamp. Their retained parent,
            // assignment and immutable initial source/path identify the row.
            worktree_identity_digest: super::super::store::digest(&(
                &worktree.id,
                &worktree.workspace_id,
                &worktree.sandbox_id,
                &worktree.owner,
                &worktree.name,
                &worktree.path,
                &worktree.branch,
                &worktree.base_commit,
                &worktree.agent_id,
            ))?,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), BridgeInvokeError> {
        if [
            self.ids.sandbox_id.as_str(),
            self.ids.workspace_id.as_str(),
            self.ids.worktree_id.as_str(),
            self.ids.agent_id.as_str(),
        ]
        .iter()
        .any(|id| !super::super::config::valid_id(id))
            || [
                &self.sandbox_created_at,
                &self.workspace_created_at,
                &self.agent_created_at,
            ]
            .iter()
            .any(|stamp| !super::model::stamp(stamp))
            || !super::model::digest(&self.agent_creation_fingerprint)
            || !super::model::digest(&self.worktree_identity_digest)
        {
            return Err(BridgeInvokeError::Corrupt);
        }
        Ok(())
    }

    pub fn resource(&self) -> ResourceTarget {
        ResourceTarget::Agent {
            agent_id: self.ids.agent_id.clone(),
        }
    }

    /// Retained replies need the original owner/tree identity and available
    /// parent records. Stopping the Agent does not withdraw an accepted reply.
    /// Removed children and retired/purged parents still require separate proof.
    pub fn require_retained_reply_target(
        &self,
        owner: &ResourceOwner,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), BridgeInvokeError> {
        if Self::capture(snapshot)? != *self
            || snapshot
                .scope(&self.resource())
                .map_err(|_| BridgeInvokeError::TargetMismatch)?
                .owner
                != *owner
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        let lineage = &snapshot.lineage;
        if !matches!(
            lineage.sandbox.status,
            SandboxStatus::Active
                | SandboxStatus::Paused
                | SandboxStatus::Pausing
                | SandboxStatus::Reviving
                | SandboxStatus::CredentialRotating
                | SandboxStatus::Rotating
        ) || lineage
            .workspace
            .as_ref()
            .is_none_or(|v| v.status != WorkspaceStatus::Ready)
            || lineage
                .worktree
                .as_ref()
                .is_none_or(|v| v.status != WorktreeStatus::Ready)
            || lineage
                .agent
                .as_ref()
                .is_none_or(|v| v.deleted_at.is_some() || v.status == AgentStatus::Deleted)
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
    pub fn require(
        &self,
        owner: &ResourceOwner,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), BridgeInvokeError> {
        self.require_retained_reply_target(owner, snapshot)?;
        if snapshot.lineage.agent.as_ref().is_none_or(|v| {
            !v.status.is_live()
                || v.requested_stop_reason.is_some()
                || !v
                    .capabilities
                    .supports(crate::RuntimeCapability::Conversation)
        }) {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        Ok(())
    }
}
