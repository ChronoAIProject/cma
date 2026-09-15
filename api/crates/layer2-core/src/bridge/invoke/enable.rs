use super::*;
use crate::{
    bridge::{BridgeError, BridgeManagementLease, BridgeRecord, BridgeSetupBlock, BridgeState},
    resource_access::{ResourceAccessSnapshot, ResourceActor},
};

/// Nonserializable grant prerequisite for one actual manager's exact enable
/// transition. This does not authorize allocation, command input or execution.
#[derive(Debug, Clone)]
pub struct BridgeInvokeEnableProof {
    grant: BridgeInvokeGrant,
    bridge: BridgeRecord,
    resources: ResourceAccessSnapshot,
    actor: ResourceActor,
    deadline: tokio::time::Instant,
}
impl BridgeInvokeEnableProof {
    pub fn grant(&self) -> &BridgeInvokeGrant {
        &self.grant
    }
    pub fn bridge(&self) -> &BridgeRecord {
        &self.bridge
    }
    pub fn resources(&self) -> &ResourceAccessSnapshot {
        &self.resources
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn require_current(&self) -> Result<(), BridgeError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeError::AuthorityUnavailable);
        }
        self.grant.require_active().map_err(enable_error)?;
        if matches!(self.bridge.state, BridgeState::Retired { .. }) {
            return Err(BridgeError::Retired);
        }
        if self.grant.pin.bridge_id != self.bridge.id
            || self.grant.owner != self.bridge.owner
            || self.grant.pin.config_revision != self.bridge.config_revision
            || self.grant.config != self.bridge.current()?.config
        {
            return Err(BridgeError::Conflict);
        }
        super::service::require_resource_owner(&self.grant, &self.resources).map_err(enable_error)
    }
    pub fn require_snapshot(
        &self,
        bridge: &BridgeRecord,
        grant: &BridgeInvokeGrant,
        resources: &ResourceAccessSnapshot,
    ) -> Result<(), BridgeError> {
        self.require_current()?;
        grant.require_active().map_err(enable_error)?;
        if bridge != &self.bridge || grant != &self.grant {
            return Err(BridgeError::Conflict);
        }
        if resources != &self.resources {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::TargetMismatch,
            ));
        }
        super::service::require_resource_owner(grant, resources).map_err(enable_error)
    }
    pub fn require_write(
        &self,
        record: &BridgeRecord,
        actor: &ResourceActor,
        expected_revision: u64,
    ) -> Result<(), BridgeError> {
        self.require_current()?;
        let mut expected = self.bridge.clone();
        expected.state = BridgeState::Enabled;
        expected.revision += 1;
        expected.updated_at = record.updated_at.clone();
        if expected_revision != self.bridge.revision || record != &expected || actor != &self.actor
        {
            return Err(BridgeError::Conflict);
        }
        Ok(())
    }
}
impl BridgeInvokeService {
    pub async fn prepare_enable(
        &self,
        management: &BridgeManagementLease,
    ) -> Result<BridgeInvokeEnableProof, BridgeError> {
        management.require_current()?;
        let candidate = self
            .current_candidate(management)
            .await
            .map_err(enable_error)?
            .ok_or(BridgeError::ConnectionRequired(
                BridgeSetupBlock::InvokeConsentRequired,
            ))?;
        let grant = self
            .store
            .grant(&candidate.id)
            .await
            .map_err(enable_error)?
            .ok_or(BridgeError::Conflict)?;
        if grant.view() != candidate {
            return Err(BridgeError::Conflict);
        }
        let resources = self
            .resources
            .resource_access_snapshot(&grant.target.resource())
            .await
            .map_err(super::service::resource_store_error)
            .map_err(enable_error)?;
        let proof = BridgeInvokeEnableProof {
            grant,
            bridge: management.record().clone(),
            resources,
            actor: management.actor().clone(),
            deadline: management.deadline(),
        };
        proof.require_current()?;
        self.definitions.validate_management(management).await?;
        Ok(proof)
    }
}
fn enable_error(error: BridgeInvokeError) -> BridgeError {
    match error {
        BridgeInvokeError::Bridge(error) => error,
        BridgeInvokeError::NotFound | BridgeInvokeError::Revoked => {
            BridgeError::ConnectionRequired(BridgeSetupBlock::InvokeConsentRequired)
        }
        BridgeInvokeError::TargetMismatch => {
            BridgeError::ConnectionRequired(BridgeSetupBlock::TargetMismatch)
        }
        BridgeInvokeError::Forbidden => BridgeError::Forbidden,
        BridgeInvokeError::Conflict => BridgeError::Conflict,
        BridgeInvokeError::IdempotencyConflict => BridgeError::IdempotencyConflict,
        BridgeInvokeError::Invalid(field) => BridgeError::InvalidConfiguration(field),
        BridgeInvokeError::Suspended => BridgeError::AdmissionStopped,
        BridgeInvokeError::Unavailable => BridgeError::AuthorityUnavailable,
        BridgeInvokeError::Corrupt => BridgeError::Corrupt,
    }
}
