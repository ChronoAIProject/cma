//! Authoritative ownership/share snapshots and compare-and-swap mutations.
//! Stores implement this against the same resource transaction boundary used
//! by Agent dispatch; historical receipts cannot substitute for current reads.

use serde::{Deserialize, Serialize};

use crate::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

use super::{ResourceActor, ResourceOwner, ResourceScope, ResourceShare};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceTarget {
    Sandbox { sandbox_id: SandboxId },
    Workspace { workspace_id: WorkspaceId },
    Worktree { worktree_id: WorktreeId },
    Agent { agent_id: AgentId },
}

/// Explicit organization evidence belongs in the atomic new-Sandbox admission.
/// This is never inferred from a subject string or a configuring human's name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationOwnershipEvidence {
    pub issuer: String,
    pub organization: SubjectId,
    pub admitted_by: ResourceActor,
    pub verified_at: String,
}

/// Additive sidecar. Missing sidecars mean Person(the aggregate's owner).
/// The Sandbox creation timestamp fences later accidental ID reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcePolicyRecord {
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub owner: ResourceOwner,
    pub organization_evidence: Option<OrganizationOwnershipEvidence>,
    pub revision: u64,
}

impl ResourcePolicyRecord {
    /// Inventory selectors must distinguish an explicit organization before
    /// pagination, including the hidden lookahead used to produce a cursor.
    pub fn personal_owner_of(
        &self,
        sandbox: &crate::sandbox::Sandbox,
    ) -> Result<bool, ResourceAccessStoreError> {
        self.validate()?;
        let (subject, personal) = match &self.owner {
            ResourceOwner::Person { subject } => (subject, true),
            ResourceOwner::Organization { subject } => (subject, false),
        };
        if self.sandbox_id != sandbox.id
            || self.sandbox_created_at != sandbox.created_at
            || subject != &sandbox.owner
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(personal)
    }

    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        let subject = match &self.owner {
            ResourceOwner::Person { subject } if self.organization_evidence.is_none() => subject,
            ResourceOwner::Organization { subject } => {
                let evidence = self
                    .organization_evidence
                    .as_ref()
                    .ok_or(ResourceAccessStoreError::Corrupt)?;
                if evidence.organization != *subject
                    || evidence.issuer.trim().is_empty()
                    || crate::seconds_from_rfc3339(&evidence.verified_at).is_none()
                    || evidence.admitted_by.ordinary_subject().is_none()
                {
                    return Err(ResourceAccessStoreError::Corrupt);
                }
                subject
            }
            _ => return Err(ResourceAccessStoreError::Corrupt),
        };
        if self.sandbox_id.as_str().trim().is_empty()
            || subject.as_str().trim().is_empty()
            || crate::seconds_from_rfc3339(&self.sandbox_created_at).is_none()
            || self.revision == 0
            || self.revision > i64::MAX as u64
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePolicyFence {
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    /// Zero denotes an absent sidecar, never an implicit organization.
    pub policy_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAccessSnapshot {
    pub lineage: super::ResourceLineage,
    pub policy: Option<ResourcePolicyRecord>,
    pub shares: Vec<ResourceShare>,
}

impl ResourceAccessSnapshot {
    pub fn scope(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceScope, ResourceAccessStoreError> {
        self.lineage.validate(target)?;
        if self.shares.len() > super::MAX_ACTIVE_RESOURCE_SHARES {
            return Err(ResourceAccessStoreError::Capacity);
        }
        if self.policy.is_none() && !self.shares.is_empty() {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        let mut share_ids = std::collections::BTreeSet::new();
        if self.shares.iter().any(|share| {
            !share_ids.insert(&share.id)
                || self
                    .policy
                    .as_ref()
                    .is_some_and(|policy| share.revision > policy.revision)
        }) {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        let root = &self.lineage.sandbox;
        let owner = match &self.policy {
            None => ResourceOwner::Person {
                subject: root.owner.clone(),
            },
            Some(policy) => {
                policy.validate()?;
                let subject = match &policy.owner {
                    ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => {
                        subject
                    }
                };
                if policy.sandbox_id != root.id
                    || policy.sandbox_created_at != root.created_at
                    || subject != &root.owner
                {
                    return Err(ResourceAccessStoreError::Corrupt);
                }
                policy.owner.clone()
            }
        };
        let scope = ResourceScope {
            sandbox_id: root.id.clone(),
            agent_id: self.lineage.agent.as_ref().map(|agent| agent.id.clone()),
            owner,
            // No isolation is inferred from a worktree, home or number of Agents.
            sharing_isolation: super::SharingIsolation::Unproved,
        };
        if self.shares.iter().any(|share| {
            !share.validate() || share.owner != scope.owner || !share.selector.covers(&scope)
        }) {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(scope)
    }

    pub fn fence(&self) -> ResourcePolicyFence {
        ResourcePolicyFence {
            sandbox_id: self.lineage.sandbox.id.clone(),
            sandbox_created_at: self.lineage.sandbox.created_at.clone(),
            policy_revision: self.policy.as_ref().map_or(0, |policy| policy.revision),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceAccessStoreError {
    #[error("resource not found")]
    NotFound,
    #[error("resource authorization changed")]
    Conflict,
    #[error("resource authorization data is inconsistent")]
    Corrupt,
    #[error("resource authorization store is unavailable")]
    Unavailable,
    #[error("interactive Agent sharing requires independent isolation or a compatible whole-Sandbox grant")]
    UnsafeInteractiveShare,
    #[error("resource share capacity reached")]
    Capacity,
    #[error("resource share command is invalid")]
    Invalid,
    #[error("idempotency key was already used for a different share command")]
    IdempotencyConflict,
}

#[async_trait::async_trait]
pub trait ResourceAccessStore: Send + Sync {
    /// Sandbox-ID ordered candidates covered by explicit organization ownership
    /// or whole-Sandbox grants. Filter/deduplicate before returning limit + 1;
    /// narrow Agent grants must never supply parent inventory candidates.
    async fn organization_sandboxes(
        &self,
        organization: &SubjectId,
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError>;

    /// Whole-Sandbox incoming grants across verified current organizations.
    /// Exclude the recipient's own personal resources and deduplicate before
    /// returning at most limit + 1 ordered snapshots, including the lookahead.
    async fn shared_sandboxes(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError>;

    /// Agent-ID ordered organization inventory, filtered before pagination and
    /// deduplicated across org ownership and overlapping grants. Return at most
    /// limit + 1 snapshots so the service can authorize the lookahead too. These
    /// are internal candidates; callers still need fresh organization authority.
    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &crate::sandbox::AgentListQuery,
        page: &crate::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError>;

    /// One snapshot includes the complete ancestry, current owner kind and only
    /// grants that cover this exact resource. The caller must authorize before
    /// exposing any returned aggregate, including parent metadata.
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError>;
}
