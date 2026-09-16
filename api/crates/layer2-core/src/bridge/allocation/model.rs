use serde::{Deserialize, Serialize};

use crate::bridge::{
    partition::{NormalizedPartitionKey, PartitionSourcePin},
    BridgeAgentTarget, BridgeConfig, BridgeId, BridgeWorkPin,
};
use crate::resource_access::{ResourceActor, ResourceOwner};

use super::{AllocationError, AllocationProfiles};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationIdentity {
    pub bridge_id: BridgeId,
    pub key: NormalizedPartitionKey,
    /// Reset lineage counter; independent from the bridge configuration revision.
    pub generation: u64,
}

impl AllocationIdentity {
    pub fn id(&self) -> Result<String, AllocationError> {
        if self.generation == 0 || self.generation > 256 {
            return Err(AllocationError::Corrupt);
        }
        Ok(format!(
            "bral_{}",
            super::hash(&("cma.bridge.allocation.v1", self))?
        ))
    }

    pub fn head_id(&self) -> Result<String, AllocationError> {
        super::hash(&("cma.bridge.allocation-head.v1", &self.bridge_id, &self.key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AllocationPlacement {
    ReuseParent {
        target: BridgeAgentTarget,
        profiles: Box<AllocationProfiles>,
        parent: super::AllocationParentIdentity,
    },
    ExistingAgent {
        target: BridgeAgentTarget,
        grant_id: String,
    },
    NewSandbox {
        target: BridgeAgentTarget,
        profiles: Box<AllocationProfiles>,
    },
}

impl AllocationPlacement {
    pub fn target(&self) -> &BridgeAgentTarget {
        match self {
            Self::ExistingAgent { target, .. }
            | Self::NewSandbox { target, .. }
            | Self::ReuseParent { target, .. } => target,
        }
    }
    pub fn profiles(&self) -> Option<&AllocationProfiles> {
        match self {
            Self::NewSandbox { profiles, .. } | Self::ReuseParent { profiles, .. } => {
                Some(profiles)
            }
            _ => None,
        }
    }
    pub fn creates_sandbox(&self) -> bool {
        matches!(self, Self::NewSandbox { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationState {
    Reserved,
    Provisioning,
    Ready,
    NeedsConnection,
    FailedRetryable,
    FailedTerminal,
    Retired,
}

/// Identity for provider reconciliation, never permission for a second create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationResourceReceipt {
    pub operation_id: String,
    pub target: BridgeAgentTarget,
    pub receipt_digest: String,
    pub profiles_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationLease {
    pub holder: String,
    pub fence: u64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationPredecessor {
    pub id: String,
    pub generation: u64,
    pub reset_by: ResourceActor,
}

/// Internal retained record. Profile snapshots may contain commands and public
/// environment values; public management must use a deliberately bounded view.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationRecord {
    pub id: String,
    pub identity: AllocationIdentity,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub config: BridgeConfig,
    pub first_source: PartitionSourcePin,
    pub placement: AllocationPlacement,
    pub operation_id: String,
    pub predecessor: Option<AllocationPredecessor>,
    pub state: AllocationState,
    pub revision: u64,
    pub lease_fence: u64,
    pub lease: Option<AllocationLease>,
    /// Once true, every takeover must reconcile the original operation first.
    pub effect_started: bool,
    pub receipt: Option<AllocationResourceReceipt>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl AllocationRecord {
    pub fn validate(&self) -> Result<(), AllocationError> {
        let valid_id = super::super::config::valid_id;
        let target = self.placement.target();
        let owner_subject = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if self.id != self.identity.id()?
            || !valid_id(owner_subject.as_str())
            || !valid_id(self.identity.bridge_id.as_str())
            || self.identity.bridge_id != self.pin.bridge_id
            || self.first_source.work_pin() != &self.pin
            || self.pin.config_revision == 0
            || self.pin.config_revision > 256
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.lease_fence > i64::MAX as u64
            || self.created_at_ms < 0
            || self.updated_at_ms < self.created_at_ms
            || self.operation_id != format!("{}:provision:v1", self.id)
            || ![
                target.sandbox_id.as_str(),
                target.workspace_id.as_str(),
                target.worktree_id.as_str(),
                target.agent_id.as_str(),
            ]
            .iter()
            .all(|s| valid_id(s))
        {
            return Err(AllocationError::Corrupt);
        }
        self.config
            .validate()
            .map_err(|_| AllocationError::Corrupt)?;
        match (&self.placement, &self.config.placement) {
            (
                AllocationPlacement::ReuseParent {
                    target,
                    profiles,
                    parent,
                },
                placement,
            ) if placement.reused_parent().is_some() => {
                profiles.validate(&self.config)?;
                let mut expected = reserved_target(&self.id)?;
                match placement {
                    crate::bridge::BridgePlacementPolicy::WorkspacePerPartition {
                        sandbox_id,
                        ..
                    } if parent.workspace_created_at.is_none() => {
                        expected.sandbox_id = sandbox_id.clone();
                    }
                    crate::bridge::BridgePlacementPolicy::WorktreeAgentPerPartition {
                        sandbox_id,
                        workspace_id,
                        ..
                    } if parent
                        .workspace_created_at
                        .as_ref()
                        .is_some_and(|at| crate::seconds_from_rfc3339(at).is_some()) =>
                    {
                        expected.sandbox_id = sandbox_id.clone();
                        expected.workspace_id = workspace_id.clone();
                    }
                    _ => return Err(AllocationError::Corrupt),
                }
                if target != &expected
                    || crate::seconds_from_rfc3339(&parent.sandbox_created_at).is_none()
                    || !(1..=16).contains(&parent.limits.max_workspaces)
                    || !(1..=16).contains(&parent.limits.max_agents_per_workspace)
                    || parent
                        .service_scope
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != parent.service_scope.len()
                {
                    return Err(AllocationError::Corrupt);
                }
            }
            (
                AllocationPlacement::ExistingAgent { target, grant_id },
                crate::bridge::BridgePlacementPolicy::ExistingAgent { target: configured },
            ) if target == configured && valid_id(grant_id) => {}
            (
                AllocationPlacement::NewSandbox { profiles, .. },
                crate::bridge::BridgePlacementPolicy::NewSandbox { .. },
            ) => {
                profiles.validate(&self.config)?;
                if target != &reserved_target(&self.id)? {
                    return Err(AllocationError::Corrupt);
                }
            }
            _ => return Err(AllocationError::Corrupt),
        }
        match (&self.predecessor, self.identity.generation) {
            (None, 1) => {}
            (Some(previous), generation) if previous.generation + 1 == generation => {
                let mut predecessor = self.identity.clone();
                predecessor.generation = previous.generation;
                if predecessor.id()? != previous.id
                    || !matches!(
                        &previous.reset_by,
                        ResourceActor::Human { .. }
                            | ResourceActor::Api {
                                delegation_id: None,
                                ..
                            }
                    )
                {
                    return Err(AllocationError::Corrupt);
                }
            }
            _ => return Err(AllocationError::Corrupt),
        }
        if self.lease.as_ref().is_some_and(|lease| {
            !valid_id(&lease.holder)
                || lease.fence == 0
                || lease.fence != self.lease_fence
                || lease.expires_at_ms <= self.updated_at_ms
        }) || (self.state == AllocationState::Provisioning && !self.effect_started)
            || (self.state == AllocationState::Ready && self.receipt.is_none())
            || (matches!(
                self.state,
                AllocationState::Ready | AllocationState::FailedTerminal | AllocationState::Retired
            ) && self.lease.is_some())
            || (self.receipt.is_some() && !self.effect_started)
        {
            return Err(AllocationError::Corrupt);
        }
        if let Some(receipt) = &self.receipt {
            if receipt.operation_id != self.operation_id
                || &receipt.target != target
                || !super::super::partition::digest(&receipt.receipt_digest)
                || receipt.profiles_digest.as_deref() != self.profiles_digest()
            {
                return Err(AllocationError::Corrupt);
            }
        }
        Ok(())
    }

    pub fn profiles_digest(&self) -> Option<&str> {
        match &self.placement {
            AllocationPlacement::ExistingAgent { .. } => None,
            AllocationPlacement::NewSandbox { profiles, .. }
            | AllocationPlacement::ReuseParent { profiles, .. } => Some(&profiles.trust_digest),
        }
    }

    pub fn require_same_partition(&self, proposed: &Self) -> Result<(), AllocationError> {
        self.validate()?;
        if self.identity.bridge_id != proposed.identity.bridge_id
            || self.identity.key != proposed.identity.key
            || self.owner != proposed.owner
        {
            return Err(AllocationError::IdentityMismatch);
        }
        if self.state == AllocationState::Retired {
            return Err(AllocationError::ResetRequired);
        }
        Ok(())
    }
}

pub(super) fn reserved_target(id: &str) -> Result<BridgeAgentTarget, AllocationError> {
    let resource = |kind| super::hash(&("cma.bridge.resource.v1", id, kind));
    Ok(BridgeAgentTarget {
        sandbox_id: crate::SandboxId(format!("sbx_{}", &resource("sandbox")?[..32])),
        workspace_id: crate::WorkspaceId(format!("wks_{}", &resource("workspace")?[..32])),
        worktree_id: crate::WorktreeId(format!("wkt_{}", &resource("worktree")?[..32])),
        agent_id: crate::AgentId(format!("agt_{}", &resource("agent")?[..32])),
    })
}
