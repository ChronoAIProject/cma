//! Resource execution authority is issued from fresh service decisions. Stored
//! registrations and receipts are evidence only; they cannot recreate a caller.

mod effects;
mod leases;
mod release;
mod revocation;
mod store;

pub use effects::*;
pub use release::*;
pub use revocation::*;
pub use store::*;

use layer3_adapter::resource_execution_protocol as wire;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;

use super::{
    AuthorizedResource, ResourceAccessError, ResourceAccessService, ResourceAccessSnapshot,
    ResourceAccessStoreError, ResourceActor, ResourceCapability, ResourceShare, ResourceTarget,
};

pub const MAX_FAMILY_MEMBERS: u32 = 64;
pub const MAX_MEMBER_EFFECTS: u32 = 128;
pub const MAX_PAGE_MEMBERS: u32 = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
    #[error("resource execution policy is disabled")]
    Disabled,
    #[error("resource execution authority is closing")]
    Closing { operation: wire::Digest },
    #[error("resource execution authority is no longer current")]
    AuthorityChanged,
    #[error("resource execution evidence does not match")]
    EvidenceMismatch,
    #[error("resource execution capacity reached")]
    Capacity,
    #[error("resource execution data is inconsistent")]
    Corrupt,
    #[error("resource execution idempotency key conflicts")]
    IdempotencyConflict,
    #[error("resource execution transaction outcome is unknown")]
    Unknown,
    #[error("resource execution store is unavailable")]
    Unavailable,
    #[error(transparent)]
    Protocol(#[from] wire::ProtocolError),
}

pub type ExecutionResult<T> = Result<T, ExecutionError>;

impl From<ResourceAccessError> for ExecutionError {
    fn from(error: ResourceAccessError) -> Self {
        match error {
            ResourceAccessError::Store(ResourceAccessStoreError::Unavailable) => Self::Unavailable,
            ResourceAccessError::Store(ResourceAccessStoreError::Corrupt) => Self::Corrupt,
            _ => Self::AuthorityChanged,
        }
    }
}

impl From<ResourceAccessStoreError> for ExecutionError {
    fn from(error: ResourceAccessStoreError) -> Self {
        ResourceAccessError::Store(error).into()
    }
}

/// Process wiring must distinguish a verified never-activated store from a
/// missing or corrupt latch. Turning off configuration cannot erase activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationStateV1 {
    NeverActivated,
    Activated { epoch: wire::Generation },
}

#[derive(Clone)]
pub struct ResourceExecutionPolicy {
    pub(super) store: Arc<dyn ResourceExecutionStore>,
    pub(super) admitting: bool,
    pub(super) epoch: wire::Generation,
}

impl ResourceExecutionPolicy {
    pub(super) fn require_admitting(&self, authority: &ExecutionAuthority) -> ExecutionResult<()> {
        if authority.epoch() != self.epoch
            || (!self.admitting && authority.basis().family().is_some())
        {
            return Err(ExecutionError::Disabled);
        }
        Ok(())
    }

    pub async fn enforce(
        store: Arc<dyn ResourceExecutionStore>,
        admitting: bool,
    ) -> ExecutionResult<Self> {
        let ActivationStateV1::Activated { epoch } = store.activation().await? else {
            return Err(ExecutionError::Disabled);
        };
        Ok(Self {
            store,
            admitting,
            epoch,
        })
    }

    pub fn admitting(&self) -> bool {
        self.admitting
    }
    pub fn epoch(&self) -> wire::Generation {
        self.epoch
    }
}

#[derive(Clone)]
pub struct ResourceExecutionCoordinator {
    pub(super) access: ResourceAccessService,
    pub(super) policy: ResourceExecutionPolicy,
}

impl ResourceExecutionCoordinator {
    pub fn new(access: ResourceAccessService, policy: ResourceExecutionPolicy) -> Self {
        Self {
            access: access.with_execution_policy(policy.clone()),
            policy,
        }
    }
}

impl ResourceAccessService {
    pub fn with_execution_policy(mut self, policy: ResourceExecutionPolicy) -> Self {
        self.execution = Some(policy);
        self
    }
}

pub(super) fn access_error(error: ExecutionError) -> ResourceAccessError {
    match error {
        ExecutionError::Unavailable | ExecutionError::Unknown => {
            ResourceAccessStoreError::Unavailable.into()
        }
        ExecutionError::Corrupt => ResourceAccessStoreError::Corrupt.into(),
        _ => ResourceAccessStoreError::Conflict.into(),
    }
}

/// Nonserializable, service-minted authority. Even a structurally valid v1
/// record must pass current principal, selected basis and lineage checks.
pub struct ExecutionAuthority {
    pub(super) authorized: AuthorizedResource,
    pub(super) access: ResourceAccessService,
    pub(super) target: ResourceTarget,
    pub(super) capability: ResourceCapability,
    pub(super) basis: wire::ExecutionBasisV1,
    pub(super) epoch: wire::Generation,
    pub(super) credential_deadline: Option<tokio::time::Instant>,
}

impl ExecutionAuthority {
    pub fn snapshot(&self) -> &ResourceAccessSnapshot {
        &self.authorized.snapshot
    }
    pub fn target(&self) -> &ResourceTarget {
        &self.target
    }
    /// The authenticated principal survives digest-only protocol attribution.
    pub fn erasure_subject(&self) -> &crate::SubjectId {
        &self.authorized.caller.subject
    }
    pub fn basis(&self) -> &wire::ExecutionBasisV1 {
        &self.basis
    }
    pub fn epoch(&self) -> wire::Generation {
        self.epoch
    }

    pub async fn validate_live(&self) -> ExecutionResult<()> {
        self.validate_current(self.snapshot())?;
        self.access
            .validate_read_standing(&self.authorized, &self.target)
            .await?;
        self.access
            .validate_binding_authority(&self.authorized)
            .await?;
        self.validate_current(self.snapshot())
    }

    pub fn validate_current(&self, snapshot: &ResourceAccessSnapshot) -> ExecutionResult<()> {
        if self
            .credential_deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.authorized
            .decision
            .require(self.capability)
            .map_err(|_| ExecutionError::AuthorityChanged)?;
        if snapshot.fence() != self.snapshot().fence()
            || snapshot.scope(&self.target)? != self.snapshot().scope(&self.target)?
            || ancestry_digest(snapshot)? != ancestry_digest(self.snapshot())?
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        if let wire::ExecutionBasisV1::Share { grant } = &self.basis {
            let share = snapshot
                .shares
                .iter()
                .find(|share| {
                    share.id == grant.family.share_id.as_str()
                        && share.revision == grant.share_revision.get()
                        && share.revoked_at.is_none()
                })
                .ok_or(ExecutionError::AuthorityChanged)?;
            if origin_digest(snapshot, share)? != grant.family.origin {
                return Err(ExecutionError::AuthorityChanged);
            }
        }
        Ok(())
    }
}

pub(super) fn digest<T: Serialize>(value: &T) -> ExecutionResult<wire::Digest> {
    let bytes = serde_json::to_vec(value).map_err(|_| ExecutionError::Corrupt)?;
    wire::Digest::new(&format!("{:x}", Sha256::digest(bytes))).map_err(Into::into)
}

pub fn origin_digest(
    snapshot: &ResourceAccessSnapshot,
    share: &ResourceShare,
) -> ExecutionResult<wire::Digest> {
    let selector_incarnation = match &share.selector {
        super::ResourceSelector::Sandbox { .. } => &snapshot.lineage.sandbox.created_at,
        super::ResourceSelector::Agent { agent_id } => {
            &snapshot
                .lineage
                .agent
                .as_ref()
                .filter(|agent| &agent.id == agent_id)
                .ok_or(ExecutionError::AuthorityChanged)?
                .created_at
        }
    };
    digest(&(
        "resource-grant-origin-v1",
        &share.owner,
        &share.selector,
        selector_incarnation,
        &snapshot.lineage.sandbox.id,
        &snapshot.lineage.sandbox.created_at,
        &share.id,
        &share.created_at,
    ))
}

pub(super) fn ancestry_digest(snapshot: &ResourceAccessSnapshot) -> ExecutionResult<wire::Digest> {
    let lineage = &snapshot.lineage;
    digest(&(
        "resource-execution-ancestry-v1",
        &lineage.sandbox.id,
        &lineage.sandbox.created_at,
        &lineage.sandbox.owner,
        lineage
            .workspace
            .as_ref()
            .map(|row| (&row.id, &row.created_at)),
        lineage.worktree.as_ref().map(|row| {
            (
                &row.id,
                &row.workspace_id,
                &row.sandbox_id,
                &row.owner,
                &row.name,
                &row.path,
                &row.branch,
                &row.base_commit,
            )
        }),
        lineage.agent.as_ref().map(|row| (&row.id, &row.created_at)),
    ))
}

pub(super) fn actor_digest(actor: &ResourceActor) -> ExecutionResult<wire::Digest> {
    digest(&("resource-execution-actor-v1", actor))
}

/// Deterministic execution digests that identify `subject`, so account erasure
/// can recognize them in serialized execution evidence (#3296): the Human and
/// undelegated Api actors, the Trigger actor for each of its Launches, and the
/// Person and Organization owners. Delegated Api actors are not enumerable.
pub fn subject_execution_digests(
    subject: &crate::SubjectId,
    launches: &[crate::LaunchId],
) -> ExecutionResult<Vec<String>> {
    let mut actors = vec![
        ResourceActor::Human {
            subject: subject.clone(),
        },
        ResourceActor::Api {
            subject: subject.clone(),
            delegation_id: None,
        },
    ];
    actors.extend(launches.iter().map(|launch_id| ResourceActor::Trigger {
        subject: subject.clone(),
        launch_id: launch_id.clone(),
    }));
    let mut values = Vec::with_capacity(actors.len() + 2);
    for actor in &actors {
        values.push(actor_digest(actor)?.as_str().to_owned());
    }
    for owner in [
        super::ResourceOwner::Person {
            subject: subject.clone(),
        },
        super::ResourceOwner::Organization {
            subject: subject.clone(),
        },
    ] {
        values.push(digest(&owner)?.as_str().to_owned());
    }
    Ok(values)
}

pub(super) fn capability(value: ResourceCapability) -> ExecutionResult<wire::ProtectedCapability> {
    use ResourceCapability as C;
    Ok(match value {
        C::ViewDetails => wire::ProtectedCapability::ReadDetails,
        C::ViewHistory => wire::ProtectedCapability::ReadHistory,
        C::ViewArtifacts => wire::ProtectedCapability::ReadArtifacts,
        C::ViewFiles => wire::ProtectedCapability::ReadFiles,
        C::Interact => wire::ProtectedCapability::Prompt,
        C::ResolveApproval => wire::ProtectedCapability::ResolveApproval,
        C::OperateAgent => wire::ProtectedCapability::OperateAgent,
        _ => return Err(ExecutionError::AuthorityChanged),
    })
}

#[cfg(test)]
mod tests;
