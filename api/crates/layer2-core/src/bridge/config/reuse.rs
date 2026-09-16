//! Deliberate collaboration inside an existing Sandbox execution boundary.
use super::*;

/// There is deliberately no default consent and no claim of per-user isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSharedTrust {
    AllParticipantsShareFilesystemProcessesAndCredentials,
}

/// Minimum capacity kept outside this Bridge's allocation-owned children.
/// Live combined counts and all Bridge reservations are checked under the
/// existing parent writer lock; these values do not resize the parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSharedCapacity {
    pub keep_other_workspaces: u8,
    pub keep_other_agents_per_workspace: u8,
}

impl BridgeSharedCapacity {
    pub fn validate(&self) -> Result<(), BridgeError> {
        if !(1..=15).contains(&self.keep_other_workspaces)
            || !(1..=15).contains(&self.keep_other_agents_per_workspace)
        {
            return Err(BridgeError::InvalidConfiguration("shared_capacity"));
        }
        Ok(())
    }
}

impl BridgePlacementPolicy {
    pub fn creates_partition_agent(&self) -> bool {
        matches!(
            self,
            Self::NewSandbox { .. }
                | Self::WorkspacePerPartition { .. }
                | Self::WorktreeAgentPerPartition { .. }
        )
    }

    pub fn reused_parent(&self) -> Option<crate::resource_access::ResourceTarget> {
        match self {
            Self::WorkspacePerPartition { sandbox_id, .. } => {
                Some(crate::resource_access::ResourceTarget::Sandbox {
                    sandbox_id: sandbox_id.clone(),
                })
            }
            Self::WorktreeAgentPerPartition { workspace_id, .. } => {
                Some(crate::resource_access::ResourceTarget::Workspace {
                    workspace_id: workspace_id.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn shared_capacity(&self) -> Option<BridgeSharedCapacity> {
        match self {
            Self::WorkspacePerPartition { capacity, .. }
            | Self::WorktreeAgentPerPartition { capacity, .. } => Some(*capacity),
            _ => None,
        }
    }

    pub(super) fn validate_reuse(&self) -> Result<(), BridgeError> {
        let (sandbox_id, workspace_id, workspace_profile, agent_profile, capacity) = match self {
            Self::WorkspacePerPartition {
                sandbox_id,
                workspace_profile,
                agent_profile,
                capacity,
                ..
            } => (
                sandbox_id,
                None,
                Some(workspace_profile),
                agent_profile,
                capacity,
            ),
            Self::WorktreeAgentPerPartition {
                sandbox_id,
                workspace_id,
                agent_profile,
                capacity,
                ..
            } => (
                sandbox_id,
                Some(workspace_id),
                None,
                agent_profile,
                capacity,
            ),
            _ => return Ok(()),
        };
        if !valid_id(sandbox_id.as_str()) || workspace_id.is_some_and(|id| !valid_id(id.as_str())) {
            return Err(BridgeError::InvalidConfiguration("target"));
        }
        if !valid_id(agent_profile.id.as_str())
            || agent_profile.revision == 0
            || workspace_profile
                .is_some_and(|profile| !valid_id(profile.id.as_str()) || profile.revision == 0)
        {
            return Err(BridgeError::InvalidConfiguration("profile_pins"));
        }
        capacity.validate()
    }
}
