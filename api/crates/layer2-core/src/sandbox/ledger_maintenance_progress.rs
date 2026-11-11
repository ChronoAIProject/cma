//! Durable early progress before physical exclusion or native work. Neither
//! action below can release serving admission or authorize an external effect.

use crate::{seconds_from_rfc3339, Caller, Capability};

use super::ledger_maintenance::LedgerMaintenancePhaseV1 as Phase;
use super::ledger_maintenance_admission::{
    LedgerMaintenanceAdmissionPolicy as Policy, LedgerMaintenanceAdmissionRefusal as Refusal,
    LedgerMaintenanceStateV1,
};
use super::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerMaintenanceEarlyAction {
    /// Record the start of draining. This does not assert that old work is drained.
    BeginDrain,
    /// Stop this early workflow, retaining the non-serving hold and original plan.
    AbortHeld,
}

#[derive(Clone)]
pub struct LedgerMaintenanceProgress {
    pub caller: Caller,
    pub expected: LedgerMaintenanceStateV1,
    pub expected_source: LedgerMaintenanceSnapshotV1,
    pub holder: String,
    pub now: String,
    pub action: LedgerMaintenanceEarlyAction,
}

pub fn plan_ledger_maintenance_progress(
    policy: Policy,
    current: &LedgerMaintenanceStateV1,
    source: &LedgerMaintenanceSnapshotV1,
    request: &LedgerMaintenanceProgress,
) -> Result<LedgerMaintenanceStateV1, Refusal> {
    if !policy.enabled {
        return Err(Refusal::Disabled);
    }
    if !((request.caller.subject == source.sandbox.owner
        && request
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes))
        || request
            .caller
            .authorization
            .allows(Capability::ControlAnySandbox))
    {
        return Err(Refusal::NotFound);
    }
    let accepted = current.accepted()?;
    if current != &request.expected
        || source != &request.expected_source
        || source != &accepted.plan().source
    {
        return Err(Refusal::Conflict);
    }
    source
        .validate_for(&current.record.binding, &request.now)
        .map_err(|_| Refusal::InvalidEvidence)?;
    let now = stamp(&request.now)?;
    if now < stamp(&current.updated_at)? {
        return Err(Refusal::InvalidEvidence);
    }
    if request.holder != current.lease.holder || now >= stamp(&current.lease.expires_at)? {
        return Err(Refusal::StaleLease);
    }
    // Early steps cannot reuse a partially applied/excluded workflow, even if
    // its phase or evidence is malformed. Recovery of those needs its own command.
    if !matches!(current.record.phase, Phase::Requested | Phase::Draining)
        || current.record.authority_receipt_sha256.is_some()
        || current.record.backup_receipt_sha256.is_some()
        || current.record.release_receipt_sha256.is_some()
    {
        return Err(Refusal::Conflict);
    }
    let phase = match (current.record.phase, request.action) {
        (Phase::Requested, LedgerMaintenanceEarlyAction::BeginDrain) => Phase::Draining,
        (Phase::Requested | Phase::Draining, LedgerMaintenanceEarlyAction::AbortHeld) => {
            Phase::AbortedHeld
        }
        _ => return Err(Refusal::Conflict),
    };
    let mut next = current.clone();
    next.record.phase = phase;
    next.record.record_revision = next
        .record
        .record_revision
        .checked_add(1)
        .ok_or(Refusal::InvalidEvidence)?;
    next.updated_at = request.now.clone();
    // Original request bytes, epoch, holder and expiry are not renewed by progress.
    next.accepted()?;
    Ok(next)
}

fn stamp(value: &str) -> Result<u64, Refusal> {
    seconds_from_rfc3339(value).ok_or(Refusal::InvalidEvidence)
}

#[cfg(test)]
#[path = "ledger_maintenance_progress_tests.rs"]
mod tests;
