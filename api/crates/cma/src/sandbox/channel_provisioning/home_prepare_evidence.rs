//! Dispatch evidence retains uncertainty; it cannot authorize physical cleanup.
use super::{
    home_provenance::ChannelHomeProvenance,
    state::{ChannelProvisioningPhase, ChannelProvisioningSnapshot},
};
use layer2_core::bridge::allocation::AllocationError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomePrepareDispatch {
    MayHaveBeenSent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomePrepareReconciliation {
    Unresolved,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomePhysicalProof {
    Unproven,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HomePhysicalCleanupCapability {
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelHomePrepareEvidence {
    pub schema_version: u16,
    pub allocation_id: String,
    pub original_provenance_sha256: String,
    pub request_sha256: String,
    pub dispatch: HomePrepareDispatch,
    pub generation_history: HomePrepareReconciliation,
    pub provider_incarnation: HomePhysicalProof,
    pub backing_storage_absence: HomePhysicalProof,
    pub physical_cleanup: HomePhysicalCleanupCapability,
}

impl ChannelHomePrepareEvidence {
    pub fn capture(original: &ChannelHomeProvenance) -> Result<Self, AllocationError> {
        // A later successful observation changes only first_receipt. It cannot
        // settle whether earlier calls created other generations or were sent.
        let mut intent = original.clone();
        intent.first_receipt = None;
        Ok(Self {
            schema_version: 1,
            allocation_id: original.allocation_id.clone(),
            original_provenance_sha256: layer2_core::profiles::content_digest(&(
                "channel-home-prepare-evidence-v1",
                intent,
            ))
            .map_err(|_| AllocationError::Corrupt)?,
            request_sha256: original.request.computed_sha256(),
            dispatch: HomePrepareDispatch::MayHaveBeenSent,
            generation_history: HomePrepareReconciliation::Unresolved,
            provider_incarnation: HomePhysicalProof::Unproven,
            backing_storage_absence: HomePhysicalProof::Unproven,
            physical_cleanup: HomePhysicalCleanupCapability::Unavailable,
        })
    }

    pub fn validate(&self, snapshot: &ChannelProvisioningSnapshot) -> Result<(), AllocationError> {
        let original = snapshot
            .home_provenance
            .as_ref()
            .ok_or(AllocationError::IdentityMismatch)?;
        self.validate_original(original)
    }

    pub fn validate_original(
        &self,
        original: &ChannelHomeProvenance,
    ) -> Result<(), AllocationError> {
        if *self != Self::capture(original)? {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }

    pub fn require_successor(
        previous: &ChannelProvisioningSnapshot,
        next: &ChannelProvisioningSnapshot,
    ) -> Result<(), AllocationError> {
        match (&previous.home_prepare_evidence, &next.home_prepare_evidence) {
            (None, None) => Ok(()),
            (None, Some(evidence))
                if previous.state.phase == ChannelProvisioningPhase::Home
                    && next.state.phase == ChannelProvisioningPhase::Home
                    && next
                        .home_provenance
                        .as_ref()
                        .is_some_and(|original| original.first_receipt.is_none()) =>
            {
                evidence.validate(next)
            }
            (Some(before), Some(after)) if before == after => Ok(()),
            _ => Err(AllocationError::IdentityMismatch),
        }
    }
}
