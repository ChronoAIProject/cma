//! E1 admission artifact. This format deliberately has no execution binding,
//! exclusion receipt, backup, archive or apply/release authority.

use layer3_adapter::runtime_ledger_maintenance::valid_sha256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::agent_ledger_recovery::LedgerRecoveryDispositionV1;
use super::ledger_maintenance_admission::{
    LedgerMaintenanceAdmissionRefusal as Refusal, LedgerMaintenanceLeaseV1,
};
use super::ledger_maintenance_plan::{
    LedgerMaintenanceSnapshotV1, MAX_LEDGER_MAINTENANCE_PLAN_BYTES,
};
use crate::{seconds_from_rfc3339, SandboxId, SubjectId};

/// Exact compatible release manifests. These references are not qualification
/// by themselves: composition must verify the deployed reader/writer floor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceFloorV1 {
    pub reader_manifest_sha256: String,
    pub writer_manifest_sha256: String,
}

impl LedgerMaintenanceFloorV1 {
    pub fn validate(&self) -> Result<(), Refusal> {
        if !valid_sha256(&self.reader_manifest_sha256)
            || !valid_sha256(&self.writer_manifest_sha256)
        {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerPreExclusionPlanV1 {
    pub format: String,
    pub operation_id: String,
    pub sandbox_id: SandboxId,
    pub home_identity: String,
    pub generation: u64,
    pub boot_id: String,
    pub roots_sha256: String,
    pub source: LedgerMaintenanceSnapshotV1,
    pub disposition: LedgerRecoveryDispositionV1,
    pub authorized_by: SubjectId,
    pub consent_receipt_sha256: String,
    pub writer_barrier_epoch: u64,
    pub compatible_floor: LedgerMaintenanceFloorV1,
    pub requested_at: String,
}

impl LedgerPreExclusionPlanV1 {
    pub fn validate(&self) -> Result<(), Refusal> {
        if self.format != "ledger-pre-exclusion-plan-v1"
            || !identity(&self.operation_id)
            || self.writer_barrier_epoch != 1
            || !valid_sha256(&self.roots_sha256)
            || !valid_sha256(&self.consent_receipt_sha256)
            || self.authorized_by != self.source.sandbox.owner
        {
            return Err(Refusal::InvalidEvidence);
        }
        self.compatible_floor.validate()?;
        stamp(&self.requested_at)?;
        self.source
            .validate_inventory(&self.requested_at)
            .map_err(|_| Refusal::InvalidEvidence)?;
        let sandbox = &self.source.sandbox;
        let runtime = sandbox
            .retained_runtime_binding()
            .ok_or(Refusal::InvalidEvidence)?;
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok());
        if self.sandbox_id != sandbox.id
            || home.as_ref() != Some(&self.home_identity)
            || self.generation == 0
            || Some(self.generation) != runtime.provision_generation
            || runtime.boot_id.as_ref() != Some(&self.boot_id)
            || !identity(&self.boot_id)
        {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(())
    }
}

/// Immutable original bytes, including whitespace. A digest is kept outside the
/// payload to avoid a self-referential hash. No V1 execution bytes are rewritten.
#[derive(Clone)]
pub struct LedgerPreExclusionArtifactV1 {
    bytes: Vec<u8>,
    sha256: String,
    plan: LedgerPreExclusionPlanV1,
}

impl LedgerPreExclusionArtifactV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, Refusal> {
        if bytes.len() > MAX_LEDGER_MAINTENANCE_PLAN_BYTES {
            return Err(Refusal::InvalidEvidence);
        }
        let plan: LedgerPreExclusionPlanV1 =
            serde_json::from_slice(bytes).map_err(|_| Refusal::InvalidEvidence)?;
        plan.validate()?;
        Ok(Self {
            bytes: bytes.to_vec(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            plan,
        })
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn plan(&self) -> &LedgerPreExclusionPlanV1 {
        &self.plan
    }
}

impl std::fmt::Debug for LedgerPreExclusionArtifactV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerPreExclusionArtifactV1")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

/// All possible phases retain the hold, including after lease expiry/restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerPreExclusionPhaseV1 {
    Requested,
    Draining,
    AbortedHeld,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerPreExclusionStateV1 {
    pub format: String,
    pub admission_sha256: String,
    pub accepted_admission_bytes: Vec<u8>,
    pub writer_barrier_epoch: u64,
    pub record_revision: u64,
    pub phase: LedgerPreExclusionPhaseV1,
    pub requested_by: SubjectId,
    pub admitted_at: String,
    pub updated_at: String,
    pub lease: LedgerMaintenanceLeaseV1,
}

impl LedgerPreExclusionStateV1 {
    pub fn accepted(&self) -> Result<LedgerPreExclusionArtifactV1, Refusal> {
        let accepted = LedgerPreExclusionArtifactV1::parse(&self.accepted_admission_bytes)?;
        if self.format != "ledger-pre-exclusion-state-v1"
            || self.admission_sha256 != accepted.sha256()
            || self.writer_barrier_epoch < accepted.plan().writer_barrier_epoch
            || self.record_revision < self.writer_barrier_epoch
            || !identity(self.requested_by.as_str())
            || stamp(&self.admitted_at)? < stamp(&accepted.plan().requested_at)?
            || stamp(&self.updated_at)? < stamp(&self.admitted_at)?
        {
            return Err(Refusal::InvalidEvidence);
        }
        validate_lease(&self.lease, &self.updated_at)?;
        Ok(accepted)
    }

    /// All ordinary writer classes, including purge, refuse. There is no
    /// time/lease/flag-derived release and no caller-supplied execution permit.
    pub fn check_writer(
        &self,
        _class: super::ledger_maintenance::LedgerWriterClass,
    ) -> Result<(), super::ledger_maintenance::LedgerMaintenanceRefusal> {
        use super::ledger_maintenance::LedgerMaintenanceRefusal;
        self.accepted()
            .map_err(|_| LedgerMaintenanceRefusal::InvalidEvidence)?;
        Err(LedgerMaintenanceRefusal::Held)
    }
}

impl std::fmt::Debug for LedgerPreExclusionStateV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerPreExclusionStateV1")
            .field("admission_sha256", &self.admission_sha256)
            .field("bytes", &self.accepted_admission_bytes.len())
            .field("writer_barrier_epoch", &self.writer_barrier_epoch)
            .field("record_revision", &self.record_revision)
            .field("phase", &self.phase)
            .finish()
    }
}

pub(super) fn validate_lease(lease: &LedgerMaintenanceLeaseV1, now: &str) -> Result<(), Refusal> {
    if !identity(&lease.holder) || stamp(&lease.expires_at)? <= stamp(now)? {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(())
}

pub(super) fn stamp(value: &str) -> Result<u64, Refusal> {
    seconds_from_rfc3339(value).ok_or(Refusal::InvalidEvidence)
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
#[path = "ledger_maintenance_pre_exclusion_tests.rs"]
mod tests;
