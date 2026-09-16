//! Permanent request/ownership evidence only. No serialized value in this
//! module permits an effect, releases capacity or proves future quiescence.
use layer2_core::{
    bridge::{
        allocation::{AllocationError, AllocationRecord},
        BridgeAgentTarget,
    },
    resource_access::{ResourceActor, ResourceOwner},
    sandbox::RuntimeBinding,
};
use serde::{Deserialize, Serialize};

mod admission;
mod removal;
mod seal;
pub(crate) use admission::{
    ChannelReuseCleanupAdmission, ChannelReuseCleanupObservation, CLEANUP_ADMISSION_FIELD,
};
pub(crate) use removal::*;
pub(crate) use seal::agent_busy;
#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseCleanupRequest {
    pub request_id: String,
    pub allocation_id: String,
    pub expected_allocation_revision: u64,
}
impl ChannelReuseCleanupRequest {
    pub(crate) fn require(&self, allocation: &AllocationRecord) -> Result<(), AllocationError> {
        self.validate()?;
        if self.allocation_id != allocation.id
            || self.expected_allocation_revision != allocation.revision
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), AllocationError> {
        if self.request_id.is_empty()
            || self.request_id.len() > 128
            || !self
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
            || self.expected_allocation_revision == 0
            || self.expected_allocation_revision > i64::MAX as u64
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelReuseCleanupHold {
    AgentBusy,
    WorkspaceBusy,
    PendingCommand,
    UncertainCommand,
    PendingReply,
    UncertainReply,
    OtherWorkspaceChildren,
    InventoryLimit,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseCleanupSeal {
    pub allocation_id: String,
    pub owner: ResourceOwner,
    pub target: BridgeAgentTarget,
    pub sandbox_created_at: String,
    pub workspace_created_at: String,
    pub agent_created_at: String,
    pub runtime: RuntimeBinding,
    pub workspace_path: String,
    pub worktree_path: String,
    pub home_path: String,
    pub workspace_owned: bool,
    pub allocation_sha256: String,
    pub creation_sha256: String,
    pub startup_sha256: String,
    pub workspace_creation_sha256: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseCleanupState {
    pub schema_version: u16,
    pub id: String,
    pub request: ChannelReuseCleanupRequest,
    pub requested_by: ResourceActor,
    pub requested_at_ms: i64,
    pub seal: ChannelReuseCleanupSeal,
    /// These are observations at request time. An empty set is not an effect
    /// permit: effects must obtain new admission, lifecycle and authority fences.
    pub holds: Vec<ChannelReuseCleanupHold>,
    pub digest: String,
}
impl ChannelReuseCleanupState {
    pub(crate) fn new(
        request: ChannelReuseCleanupRequest,
        requested_by: ResourceActor,
        seal: ChannelReuseCleanupSeal,
        holds: Vec<ChannelReuseCleanupHold>,
        requested_at_ms: i64,
    ) -> Result<Self, AllocationError> {
        let mut value = Self {
            schema_version: 1,
            id: Self::id(&seal.allocation_id),
            request,
            requested_by,
            requested_at_ms,
            seal,
            holds,
            digest: String::new(),
        };
        value.digest = value.compute_digest()?;
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn id(allocation_id: &str) -> String {
        format!("{allocation_id}:reuse-cleanup-request:v1")
    }
    fn compute_digest(&self) -> Result<String, AllocationError> {
        digest(&(
            self.schema_version,
            &self.id,
            &self.request,
            &self.requested_by,
            self.requested_at_ms,
            &self.seal,
            &self.holds,
        ))
    }
    pub(crate) fn validate(&self) -> Result<(), AllocationError> {
        self.request.validate()?;
        self.seal.validate()?;
        if self.schema_version != 1
            || self.id != Self::id(&self.seal.allocation_id)
            || self.request.allocation_id != self.seal.allocation_id
            || self.requested_at_ms < 0
            || self.request.expected_allocation_revision == 0
            || self.holds.windows(2).any(|pair| pair[0] >= pair[1])
            || self.digest != self.compute_digest()?
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}

pub(super) fn digest(value: &impl Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(value).map_err(|_| AllocationError::Corrupt)
}
