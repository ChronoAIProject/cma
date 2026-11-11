//! Pure admission/takeover decisions for one durable per-home transaction.
//! The store compares the supplied complete current snapshot and record revision
//! in the same transaction that writes the hold. No provider call occurs here.

use crate::{seconds_from_rfc3339, Caller, Capability, SubjectId};
use serde::{Deserialize, Serialize};

use super::ledger_maintenance::{
    LedgerMaintenancePhaseV1, LedgerMaintenanceRecordV1, LEDGER_MAINTENANCE_FORMAT,
};
use super::ledger_maintenance_plan::{
    LedgerMaintenancePlanArtifactV1, LedgerMaintenanceSnapshotV1,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LedgerMaintenanceAdmissionPolicy {
    /// Composition enables this only after the shared writer matrix is integrated.
    /// This gate does not grant physical exclusion or release authority.
    pub enabled: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceLeaseV1 {
    pub holder: String,
    pub expires_at: String,
}

/// Stored under the parent row, atomically with source fences and audit evidence.
/// The original accepted bytes remain unchanged through phase progress/takeover.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceStateV1 {
    pub format: String,
    pub record: LedgerMaintenanceRecordV1,
    pub accepted_plan_bytes: Vec<u8>,
    pub requested_by: SubjectId,
    pub admitted_at: String,
    pub updated_at: String,
    pub lease: LedgerMaintenanceLeaseV1,
}

impl LedgerMaintenanceStateV1 {
    pub fn accepted(
        &self,
    ) -> Result<LedgerMaintenancePlanArtifactV1, LedgerMaintenanceAdmissionRefusal> {
        self.record.validate().map_err(|_| invalid())?;
        let accepted = LedgerMaintenancePlanArtifactV1::parse(&self.accepted_plan_bytes)
            .map_err(|_| invalid())?;
        let mut current_binding = accepted.plan().binding.clone();
        current_binding.maintenance_epoch = self.record.binding.maintenance_epoch;
        let admitted = stamp(&self.admitted_at)?;
        let updated = stamp(&self.updated_at)?;
        if self.format != "ledger-maintenance-state-v1"
            || self.record.binding != current_binding
            || self.record.binding.maintenance_epoch < accepted.plan().binding.maintenance_epoch
            || self.record.plan_sha256 != accepted.sha256()
            || !identity(self.requested_by.as_str())
            || !identity(&self.lease.holder)
            || admitted < stamp(&accepted.plan().requested_at)?
            || updated < admitted
            || stamp(&self.lease.expires_at)? <= updated
        {
            return Err(invalid());
        }
        Ok(accepted)
    }
}

impl std::fmt::Debug for LedgerMaintenanceStateV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerMaintenanceStateV1")
            .field("record", &self.record)
            .field("accepted_plan_bytes", &self.accepted_plan_bytes.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerMaintenanceAdmissionRefusal {
    #[error("ledger maintenance admission is disabled")]
    Disabled,
    #[error("ledger maintenance resource is not available to this caller")]
    NotFound,
    #[error("ledger maintenance evidence is invalid")]
    InvalidEvidence,
    #[error("ledger maintenance source or operation changed")]
    Conflict,
    #[error("ledger maintenance lease is stale or still owned")]
    StaleLease,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerMaintenanceAdmissionV1 {
    Persist(Box<LedgerMaintenanceStateV1>),
    Replay(Box<LedgerMaintenanceStateV1>),
}

pub struct LedgerMaintenanceAdmissionRequest<'a> {
    pub caller: &'a Caller,
    pub accepted: &'a LedgerMaintenancePlanArtifactV1,
    pub lease: LedgerMaintenanceLeaseV1,
    pub now: &'a str,
}

pub fn plan_ledger_maintenance_admission(
    policy: LedgerMaintenanceAdmissionPolicy,
    current: &LedgerMaintenanceSnapshotV1,
    existing: Option<&LedgerMaintenanceStateV1>,
    request: LedgerMaintenanceAdmissionRequest<'_>,
) -> Result<LedgerMaintenanceAdmissionV1, LedgerMaintenanceAdmissionRefusal> {
    if !policy.enabled {
        return Err(LedgerMaintenanceAdmissionRefusal::Disabled);
    }
    authorize(request.caller, current)?;
    let plan = request.accepted.plan();
    if current.sandbox.id != plan.source.sandbox.id
        || current.sandbox.owner != plan.source.sandbox.owner
    {
        return Err(LedgerMaintenanceAdmissionRefusal::Conflict);
    }
    if let Some(existing) = existing {
        existing.accepted()?;
        if existing.accepted_plan_bytes != request.accepted.original_bytes() {
            return Err(LedgerMaintenanceAdmissionRefusal::Conflict);
        }
        // Lost replies are observations of the same durable operation, not
        // new admission or automatic lease takeover. No audit is appended twice.
        return Ok(LedgerMaintenanceAdmissionV1::Replay(Box::new(
            existing.clone(),
        )));
    }
    if current != &plan.source || plan.binding.maintenance_epoch != 1 {
        return Err(LedgerMaintenanceAdmissionRefusal::Conflict);
    }
    validate_lease(&request.lease, request.now)?;
    let next = LedgerMaintenanceStateV1 {
        format: "ledger-maintenance-state-v1".into(),
        record: LedgerMaintenanceRecordV1 {
            format: LEDGER_MAINTENANCE_FORMAT.into(),
            binding: plan.binding.clone(),
            plan_sha256: request.accepted.sha256().into(),
            record_revision: 1,
            phase: LedgerMaintenancePhaseV1::Requested,
            authority_receipt_sha256: None,
            backup_receipt_sha256: None,
            release_receipt_sha256: None,
        },
        accepted_plan_bytes: request.accepted.original_bytes().to_vec(),
        requested_by: request.caller.subject.clone(),
        admitted_at: request.now.into(),
        updated_at: request.now.into(),
        lease: request.lease,
    };
    next.accepted()?;
    Ok(LedgerMaintenanceAdmissionV1::Persist(Box::new(next)))
}

pub struct LedgerMaintenanceTakeoverRequest<'a> {
    pub caller: &'a Caller,
    pub expected: &'a LedgerMaintenanceStateV1,
    pub expected_source: &'a LedgerMaintenanceSnapshotV1,
    pub lease: LedgerMaintenanceLeaseV1,
    pub now: &'a str,
}

pub fn plan_ledger_maintenance_takeover(
    current: &LedgerMaintenanceStateV1,
    source: &LedgerMaintenanceSnapshotV1,
    request: LedgerMaintenanceTakeoverRequest<'_>,
) -> Result<LedgerMaintenanceStateV1, LedgerMaintenanceAdmissionRefusal> {
    authorize(request.caller, source)?;
    let accepted = current.accepted()?;
    if current != request.expected
        || source != request.expected_source
        || source != &accepted.plan().source
        || source.sandbox.id != accepted.plan().source.sandbox.id
        || source.sandbox.owner != accepted.plan().source.sandbox.owner
    {
        return Err(LedgerMaintenanceAdmissionRefusal::Conflict);
    }
    source
        .validate_for(&current.record.binding, request.now)
        .map_err(|_| invalid())?;
    // No timeout can release or reopen a restored/aborted home. The dedicated
    // restore/release coordinator must supply its own separately qualified step.
    if matches!(
        current.record.phase,
        LedgerMaintenancePhaseV1::Released
            | LedgerMaintenancePhaseV1::RestoreHeld
            | LedgerMaintenancePhaseV1::AbortedHeld
    ) || stamp(request.now)? < stamp(&current.lease.expires_at)?
    {
        return Err(LedgerMaintenanceAdmissionRefusal::StaleLease);
    }
    validate_lease(&request.lease, request.now)?;
    let mut next = current.clone();
    next.record.binding.maintenance_epoch = current
        .record
        .binding
        .maintenance_epoch
        .checked_add(1)
        .ok_or_else(invalid)?;
    next.record.record_revision = current
        .record
        .record_revision
        .checked_add(1)
        .ok_or_else(invalid)?;
    next.lease = request.lease;
    next.updated_at = request.now.into();
    next.accepted()?;
    Ok(next)
}

fn authorize(
    caller: &Caller,
    current: &LedgerMaintenanceSnapshotV1,
) -> Result<(), LedgerMaintenanceAdmissionRefusal> {
    if (caller.subject == current.sandbox.owner
        && caller.authorization.allows(Capability::UseOwnSandboxes))
        || caller.authorization.allows(Capability::ControlAnySandbox)
    {
        Ok(())
    } else {
        Err(LedgerMaintenanceAdmissionRefusal::NotFound)
    }
}

fn validate_lease(
    lease: &LedgerMaintenanceLeaseV1,
    now: &str,
) -> Result<(), LedgerMaintenanceAdmissionRefusal> {
    if !identity(&lease.holder) || stamp(&lease.expires_at)? <= stamp(now)? {
        return Err(invalid());
    }
    Ok(())
}
fn stamp(value: &str) -> Result<u64, LedgerMaintenanceAdmissionRefusal> {
    seconds_from_rfc3339(value).ok_or_else(invalid)
}
fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn invalid() -> LedgerMaintenanceAdmissionRefusal {
    LedgerMaintenanceAdmissionRefusal::InvalidEvidence
}

#[cfg(test)]
#[path = "ledger_maintenance_admission_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ledger_maintenance_test_support.rs"]
pub(crate) mod test_support;
