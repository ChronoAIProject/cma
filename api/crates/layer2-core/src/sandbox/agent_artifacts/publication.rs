use std::time::Duration;

use super::{ArtifactBinding, StoredAgentArtifact};
use crate::resource_access::{
    AccessError, AuthorizedResource, ResourceAccessError, ResourceAccessSnapshot,
    ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};

/// Opaque live authority created only by the publication service. Persisted
/// audit cannot be reconstructed into this proof or renew its deadline.
pub struct ArtifactPublication {
    pub(super) authorized: AuthorizedResource,
    pub(super) record: StoredAgentArtifact,
    pub(super) deadline: tokio::time::Instant,
}
impl ArtifactPublication {
    pub(super) const LIFETIME: Duration = Duration::from_secs(15);
    pub fn record(&self) -> &StoredAgentArtifact {
        &self.record
    }
    pub fn target(&self) -> ResourceTarget {
        ResourceTarget::Agent {
            agent_id: self.record.binding.agent_id.clone(),
        }
    }
    pub fn require_current(&self) -> Result<(), ResourceAccessError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AccessError::RevalidationRequired.into());
        }
        self.authorized
            .decision
            .require(ResourceCapability::ManageSharing)?;
        Ok(())
    }
    pub fn validate_snapshot(
        &self,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        if snapshot.fence() != self.authorized.snapshot.fence()
            || ArtifactBinding::from_snapshot(snapshot)? != self.record.binding
            || snapshot.scope(&self.target())? != self.authorized.snapshot.scope(&self.target())?
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }
    pub fn replay(
        &self,
        record: StoredAgentArtifact,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        self.require_current()?;
        record.content()?;
        if record.binding != self.record.binding
            || record.metadata.id != self.record.metadata.id
            || record.metadata.name != self.record.metadata.name
            || record.metadata.media_type != self.record.metadata.media_type
            || record.content_base64 != self.record.content_base64
            || record.authorization.actor != self.record.authorization.actor
        {
            return Err(ResourceAccessStoreError::IdempotencyConflict.into());
        }
        Ok(record)
    }
}
