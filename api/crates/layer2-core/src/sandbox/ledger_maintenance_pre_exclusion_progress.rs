//! Pure E1 decisions. Persistence must compare the complete source, write the
//! parent barrier and retain the immutable audit in the same transaction.

use super::ledger_maintenance_admission::{
    LedgerMaintenanceAdmissionRefusal as Refusal, LedgerMaintenanceLeaseV1,
};
use super::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;
use super::ledger_maintenance_pre_exclusion::{
    stamp, validate_lease, LedgerMaintenanceFloorV1, LedgerPreExclusionArtifactV1,
    LedgerPreExclusionPhaseV1 as Phase, LedgerPreExclusionStateV1 as State,
};
use crate::{Caller, Capability};

/// No environment flag is introduced by this unregistered contract. Composition
/// supplies the exact independently qualified floor only after full integration.
#[derive(Clone, Debug, Default)]
pub struct LedgerPreExclusionPolicy {
    pub enabled: bool,
    pub qualified_floor: Option<LedgerMaintenanceFloorV1>,
}

#[derive(Clone)]
pub struct LedgerPreExclusionAdmission {
    pub caller: Caller,
    pub accepted: LedgerPreExclusionArtifactV1,
    pub lease: LedgerMaintenanceLeaseV1,
    pub now: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerPreExclusionAdmissionDecision {
    Persist(Box<State>),
    Replay(Box<State>),
}

pub fn plan_pre_exclusion_admission(
    policy: &LedgerPreExclusionPolicy,
    source: &LedgerMaintenanceSnapshotV1,
    existing: Option<&State>,
    request: &LedgerPreExclusionAdmission,
) -> Result<LedgerPreExclusionAdmissionDecision, Refusal> {
    authorize(&request.caller, source)?;
    qualify_floor(policy, &request.accepted.plan().compatible_floor)?;
    let plan = request.accepted.plan();
    if source.sandbox.id != plan.sandbox_id || source.sandbox.owner != plan.authorized_by {
        return Err(Refusal::Conflict);
    }
    if let Some(existing) = existing {
        existing.accepted()?;
        if existing.accepted_admission_bytes != request.accepted.original_bytes() {
            return Err(Refusal::Conflict);
        }
        // The persistence adapter returns the original admission result from its
        // immutable audit, not a newer lease/phase or an invented fresh snapshot.
        return Ok(LedgerPreExclusionAdmissionDecision::Replay(Box::new(
            existing.clone(),
        )));
    }
    if source != &plan.source {
        return Err(Refusal::Conflict);
    }
    validate_lease(&request.lease, &request.now)?;
    let next = State {
        format: "ledger-pre-exclusion-state-v1".into(),
        admission_sha256: request.accepted.sha256().into(),
        accepted_admission_bytes: request.accepted.original_bytes().to_vec(),
        writer_barrier_epoch: plan.writer_barrier_epoch,
        record_revision: 1,
        phase: Phase::Requested,
        requested_by: request.caller.subject.clone(),
        admitted_at: request.now.clone(),
        updated_at: request.now.clone(),
        lease: request.lease.clone(),
    };
    next.accepted()?;
    Ok(LedgerPreExclusionAdmissionDecision::Persist(Box::new(next)))
}

#[derive(Clone)]
pub enum LedgerPreExclusionAction {
    BeginDrain,
    AbortHeld,
    /// A new epoch grants only early progress. Physical exclusion must be
    /// acquired and qualified anew before a separate execution transition.
    Takeover {
        lease: LedgerMaintenanceLeaseV1,
    },
}

#[derive(Clone)]
pub struct LedgerPreExclusionProgress {
    pub caller: Caller,
    pub expected: State,
    pub expected_source: LedgerMaintenanceSnapshotV1,
    pub holder: String,
    pub now: String,
    pub action: LedgerPreExclusionAction,
}

pub fn plan_pre_exclusion_progress(
    policy: &LedgerPreExclusionPolicy,
    current: &State,
    source: &LedgerMaintenanceSnapshotV1,
    request: &LedgerPreExclusionProgress,
) -> Result<State, Refusal> {
    authorize(&request.caller, source)?;
    let accepted = current.accepted()?;
    qualify_floor(policy, &accepted.plan().compatible_floor)?;
    if current != &request.expected
        || source != &request.expected_source
        || source != &accepted.plan().source
    {
        return Err(Refusal::Conflict);
    }
    let now = stamp(&request.now)?;
    if now < stamp(&current.updated_at)? {
        return Err(Refusal::InvalidEvidence);
    }
    if current.phase == Phase::AbortedHeld {
        return Err(Refusal::Conflict);
    }
    let mut next = current.clone();
    match &request.action {
        LedgerPreExclusionAction::Takeover { lease } => {
            if now < stamp(&current.lease.expires_at)? {
                return Err(Refusal::StaleLease);
            }
            validate_lease(lease, &request.now)?;
            if request.holder != lease.holder {
                return Err(Refusal::StaleLease);
            }
            next.writer_barrier_epoch = next
                .writer_barrier_epoch
                .checked_add(1)
                .ok_or(Refusal::InvalidEvidence)?;
            next.lease = lease.clone();
        }
        action => {
            if request.holder != current.lease.holder || now >= stamp(&current.lease.expires_at)? {
                return Err(Refusal::StaleLease);
            }
            next.phase = match (current.phase, action) {
                (Phase::Requested, LedgerPreExclusionAction::BeginDrain) => Phase::Draining,
                (Phase::Requested | Phase::Draining, LedgerPreExclusionAction::AbortHeld) => {
                    Phase::AbortedHeld
                }
                _ => return Err(Refusal::Conflict),
            };
        }
    }
    next.record_revision = next
        .record_revision
        .checked_add(1)
        .ok_or(Refusal::InvalidEvidence)?;
    next.updated_at = request.now.clone();
    next.accepted()?;
    Ok(next)
}

fn qualify_floor(
    policy: &LedgerPreExclusionPolicy,
    floor: &LedgerMaintenanceFloorV1,
) -> Result<(), Refusal> {
    if !policy.enabled {
        return Err(Refusal::Disabled);
    }
    floor.validate()?;
    if policy.qualified_floor.as_ref() != Some(floor) {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(())
}

fn authorize(caller: &Caller, source: &LedgerMaintenanceSnapshotV1) -> Result<(), Refusal> {
    if (caller.subject == source.sandbox.owner
        && caller.authorization.allows(Capability::UseOwnSandboxes))
        || caller.authorization.allows(Capability::ControlAnySandbox)
    {
        Ok(())
    } else {
        Err(Refusal::NotFound)
    }
}
