//! A derived explanation of retained work, never deletion or release authority.
use super::model::ChannelRetirementSnapshot;
use crate::sandbox::channel_provisioning::home_prepare_evidence::{
    HomePhysicalCleanupCapability, HomePhysicalProof, HomePrepareDispatch,
    HomePrepareReconciliation,
};
use layer2_core::{bridge::allocation::AllocationError, SandboxId};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RetentionAuditIdentity {
    pub id: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RetentionEvidenceIdentity {
    pub allocation_id: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub operation_id: String,
    pub tree_fingerprint: String,
    pub storage_retention_sha256: String,
    pub retirement_state_sha256: String,
    pub storage_audit: RetentionAuditIdentity,
    pub original_home_provenance_sha256: Option<String>,
    pub home_prepare_evidence_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetainedHome {
    NotConfigured,
    Retained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinalRetentionHoldReason {
    OriginalHomeProvenanceMissing,
    HomePrepareEvidenceMissing,
    OriginalHomeReceiptUnbound,
    HomePrepareDispatchUnresolved,
    HomeGenerationHistoryUnresolved,
    ProviderIncarnationUnproven,
    BackingStorageAbsenceUnproven,
    PhysicalHomeCleanupUnavailable,
    ChannelHistoryPolicyRequired,
    ReaderJanitorCompatibilityRequired,
}

impl FinalRetentionHoldReason {
    fn code(self) -> &'static str {
        match self {
            Self::OriginalHomeProvenanceMissing => "original_home_missing",
            Self::HomePrepareEvidenceMissing => "prepare_evidence_missing",
            Self::OriginalHomeReceiptUnbound => "first_receipt_unbound",
            Self::HomePrepareDispatchUnresolved => "dispatch_unresolved",
            Self::HomeGenerationHistoryUnresolved => "generation_history_unresolved",
            Self::ProviderIncarnationUnproven => "provider_incarnation_unproven",
            Self::BackingStorageAbsenceUnproven => "backing_absence_unproven",
            Self::PhysicalHomeCleanupUnavailable => "physical_cleanup_unavailable",
            Self::ChannelHistoryPolicyRequired => "history_policy_required",
            Self::ReaderJanitorCompatibilityRequired => "reader_janitor_proof_required",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct FinalRetentionAssessment {
    pub identity: RetentionEvidenceIdentity,
    pub home: RetainedHome,
    pub reasons: Vec<FinalRetentionHoldReason>,
}

impl FinalRetentionAssessment {
    pub fn capture(snapshot: &ChannelRetirementSnapshot) -> Result<Self, AllocationError> {
        snapshot.validate()?;
        let retention = snapshot
            .retention
            .as_ref()
            .ok_or(AllocationError::InvalidTransition)?;
        let audit = snapshot
            .retention_audit
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        let mut reasons = Vec::new();
        let home = if retention.home.is_some() {
            use FinalRetentionHoldReason::*;
            match &snapshot.home_provenance {
                None => reasons.push(OriginalHomeProvenanceMissing),
                Some(original) if original.first_receipt.is_none() => {
                    reasons.push(OriginalHomeReceiptUnbound);
                }
                Some(_) => (),
            }
            match &snapshot.home_prepare_evidence {
                None => reasons.push(HomePrepareEvidenceMissing),
                Some(evidence) => {
                    // Exhaustive matches require a deliberate assessment if a
                    // future protocol supplies stronger proof. No observation
                    // here rewrites the immutable original uncertainty.
                    match evidence.dispatch {
                        HomePrepareDispatch::MayHaveBeenSent => {
                            reasons.push(HomePrepareDispatchUnresolved);
                        }
                    }
                    match evidence.generation_history {
                        HomePrepareReconciliation::Unresolved => {
                            reasons.push(HomeGenerationHistoryUnresolved);
                        }
                    }
                    match evidence.provider_incarnation {
                        HomePhysicalProof::Unproven => reasons.push(ProviderIncarnationUnproven),
                    }
                    match evidence.backing_storage_absence {
                        HomePhysicalProof::Unproven => reasons.push(BackingStorageAbsenceUnproven),
                    }
                    match evidence.physical_cleanup {
                        HomePhysicalCleanupCapability::Unavailable => {
                            reasons.push(PhysicalHomeCleanupUnavailable);
                        }
                    }
                }
            }
            RetainedHome::Retained
        } else {
            // Retention validation also requires that no home was configured.
            RetainedHome::NotConfigured
        };
        // Generic Sandbox grace and inbox/reply-secret retention do not supply
        // a Channel history disposition. No supported/rollback reader proof is
        // wired into this protocol yet. Neither requirement is an approval flag.
        reasons.push(FinalRetentionHoldReason::ChannelHistoryPolicyRequired);
        reasons.push(FinalRetentionHoldReason::ReaderJanitorCompatibilityRequired);
        Ok(Self {
            identity: RetentionEvidenceIdentity {
                allocation_id: snapshot.allocation.id.clone(),
                sandbox_id: snapshot.sandbox.id.clone(),
                sandbox_created_at: snapshot.sandbox.created_at.clone(),
                operation_id: snapshot.allocation.operation_id.clone(),
                tree_fingerprint: snapshot.tree.fingerprint.clone(),
                storage_retention_sha256: retention.digest()?,
                retirement_state_sha256: digest(&snapshot.state)?,
                storage_audit: audit.clone(),
                original_home_provenance_sha256: snapshot
                    .home_provenance
                    .as_ref()
                    .map(digest)
                    .transpose()?,
                home_prepare_evidence_sha256: snapshot
                    .home_prepare_evidence
                    .as_ref()
                    .map(digest)
                    .transpose()?,
            },
            home,
            reasons,
        })
    }

    pub fn hold_detail(&self) -> String {
        // Only fixed codes leave this assessment. Root/provider identities,
        // requests, human subjects and audit data remain internal.
        self.reasons
            .iter()
            .map(|reason| reason.code())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn digest(value: &impl Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(&("channel-final-retention-evidence-v1", value))
        .map_err(|_| AllocationError::Corrupt)
}
