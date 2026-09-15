//! Permanent closure of new turns, separate from current cleanup observations.
use super::*;
use layer2_core::AgentId;

pub(crate) const CLEANUP_ADMISSION_FIELD: &str = "channel_reuse_cleanup_admission";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseCleanupAdmission {
    pub schema_version: u16,
    pub id: String,
    pub request_id: String,
    pub request_sha256: String,
    pub agent_id: AgentId,
    pub closed_by: ResourceActor,
    pub closed_at_ms: i64,
    pub digest: String,
}
impl ChannelReuseCleanupAdmission {
    pub(crate) fn new(
        request: &ChannelReuseCleanupState,
        closed_by: ResourceActor,
        closed_at_ms: i64,
    ) -> Result<Self, AllocationError> {
        let mut value = Self {
            schema_version: 1,
            id: Self::id(&request.id),
            request_id: request.id.clone(),
            request_sha256: request.digest.clone(),
            agent_id: request.seal.target.agent_id.clone(),
            closed_by,
            closed_at_ms,
            digest: String::new(),
        };
        value.digest = value.compute_digest()?;
        value.require(request)?;
        Ok(value)
    }
    pub(crate) fn id(request_id: &str) -> String {
        format!("{request_id}:admission:v1")
    }
    fn compute_digest(&self) -> Result<String, AllocationError> {
        digest(&(
            self.schema_version,
            &self.id,
            &self.request_id,
            &self.request_sha256,
            &self.agent_id,
            &self.closed_by,
            self.closed_at_ms,
        ))
    }
    pub(crate) fn require(
        &self,
        request: &ChannelReuseCleanupState,
    ) -> Result<(), AllocationError> {
        request.validate()?;
        if self.schema_version != 1
            || self.id != Self::id(&request.id)
            || self.request_id != request.id
            || self.request_sha256 != request.digest
            || self.agent_id != request.seal.target.agent_id
            || self.closed_by != request.requested_by
            || self.closed_at_ms < request.requested_at_ms
            || self.digest != self.compute_digest()?
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}

/// Current observations only. This value cannot be restored as authority and
/// grants no runtime operation, lifecycle exclusion, reopen or capacity release.
pub(crate) struct ChannelReuseCleanupObservation {
    pub admission: ChannelReuseCleanupAdmission,
    pub holds: Vec<ChannelReuseCleanupHold>,
    pub runtime: RuntimeBinding,
    pub sandbox_revision: u64,
    pub workspace_revision: u64,
    pub agent_revision: u64,
    pub observed_at_ms: i64,
}
