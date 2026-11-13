//! Durable maintenance record and deny-by-default writer decisions.
//!
//! These decisions are necessary checks inside the writer's existing transaction;
//! a read followed by an unguarded write does not implement this contract.

use layer3_adapter::runtime_ledger_maintenance::{valid_sha256, LedgerBindingV1};
use serde::{Deserialize, Serialize};

pub const LEDGER_MAINTENANCE_FORMAT: &str = "ledger-maintenance-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMaintenancePhaseV1 {
    Requested,
    Draining,
    Excluded,
    BackedUp,
    Applying,
    HeldVerified,
    Released,
    RestoreHeld,
    AbortedHeld,
}

impl LedgerMaintenancePhaseV1 {
    /// Phase grammar only; never substitutes for authenticated evidence or CAS.
    pub fn accepts_successor(self, next: Self) -> bool {
        use LedgerMaintenancePhaseV1::*;
        matches!(
            (self, next),
            (Requested, Draining)
                | (Draining, Excluded)
                | (Excluded, BackedUp)
                | (BackedUp, Applying)
                | (Applying, HeldVerified)
                | (HeldVerified, Released)
                | (Applying | HeldVerified, RestoreHeld)
                | (
                    Requested
                        | Draining
                        | Excluded
                        | BackedUp
                        | Applying
                        | HeldVerified
                        | RestoreHeld,
                    AbortedHeld
                )
        )
    }
}

/// Private persistence schema, not a public request or a release permit.
/// Accepted plan/artifact bytes must be retained separately under their hashes.
/// There is deliberately no lease/time-derived serving state.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceRecordV1 {
    pub format: String,
    pub binding: LedgerBindingV1,
    pub plan_sha256: String,
    pub record_revision: u64,
    pub phase: LedgerMaintenancePhaseV1,
    pub authority_receipt_sha256: Option<String>,
    pub backup_receipt_sha256: Option<String>,
    pub release_receipt_sha256: Option<String>,
}

impl LedgerMaintenanceRecordV1 {
    pub fn validate(&self) -> Result<(), LedgerMaintenanceRefusal> {
        use LedgerMaintenancePhaseV1::*;
        if self.format != LEDGER_MAINTENANCE_FORMAT
            || self.binding.validate().is_err()
            || !valid_sha256(&self.plan_sha256)
            || self.record_revision == 0
            || [
                &self.authority_receipt_sha256,
                &self.backup_receipt_sha256,
                &self.release_receipt_sha256,
            ]
            .into_iter()
            .flatten()
            .any(|hash| !valid_sha256(hash))
            || (matches!(
                self.phase,
                Excluded | BackedUp | Applying | HeldVerified | Released | RestoreHeld
            ) && self.authority_receipt_sha256.is_none())
            || (matches!(
                self.phase,
                BackedUp | Applying | HeldVerified | Released | RestoreHeld
            ) && self.backup_receipt_sha256.is_none())
            || ((self.phase == Released) != self.release_receipt_sha256.is_some())
        {
            return Err(LedgerMaintenanceRefusal::InvalidEvidence);
        }
        Ok(())
    }
}

impl std::fmt::Debug for LedgerMaintenanceRecordV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerMaintenanceRecordV1")
            .field("binding", &self.binding)
            .field("record_revision", &self.record_revision)
            .field("phase", &self.phase)
            .finish()
    }
}

/// Exhaustive integration matrix. Every class checks the same durable hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerWriterClass {
    OwnerInput,
    OwnerControl,
    Creation,
    AgentWorker,
    WorkspaceWorker,
    Recorder,
    Checkpoint,
    Output,
    Reconciliation,
    Recovery,
    Rotation,
    Revival,
    Credentials,
    Retirement,
    HomeCleanup,
    Purge,
    Administration,
    Provider,
}

/// Absence must come from the authoritative transaction. Missing/unavailable,
/// malformed or incompletely read authority is Unknown, never KnownAbsent.
pub enum LedgerMaintenanceObservation<'a> {
    KnownAbsent,
    Present(&'a LedgerMaintenanceRecordV1),
    Unknown,
}

/// The new writer's exact accepted binding; old jobs cannot inherit a new epoch.
pub struct LedgerWriterFence<'a> {
    pub binding: &'a LedgerBindingV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerMaintenanceRefusal {
    #[error("ledger maintenance authority is unknown")]
    UnknownAuthority,
    #[error("ledger maintenance evidence is invalid")]
    InvalidEvidence,
    #[error("ledger maintenance retains the serving hold")]
    Held,
    #[error("ledger writer does not carry the released binding")]
    StaleWriter,
}

/// Ok means only that maintenance does not block this writer. Normal ownership,
/// lifecycle, retention and worker fencing still apply, in the same transaction.
/// This function cannot release maintenance or mint runtime exclusion authority.
pub fn check_ledger_writer(
    observation: LedgerMaintenanceObservation<'_>,
    _class: LedgerWriterClass,
    fence: Option<LedgerWriterFence<'_>>,
) -> Result<(), LedgerMaintenanceRefusal> {
    let current = match observation {
        LedgerMaintenanceObservation::Unknown => {
            return Err(LedgerMaintenanceRefusal::UnknownAuthority);
        }
        LedgerMaintenanceObservation::KnownAbsent => return Ok(()),
        LedgerMaintenanceObservation::Present(current) => current,
    };
    current.validate()?;
    if current.phase != LedgerMaintenancePhaseV1::Released {
        return Err(LedgerMaintenanceRefusal::Held);
    }
    let fence = fence.ok_or(LedgerMaintenanceRefusal::StaleWriter)?;
    fence
        .binding
        .validate_for(&current.binding)
        .map_err(|_| LedgerMaintenanceRefusal::StaleWriter)
}

#[cfg(test)]
#[path = "ledger_maintenance_tests.rs"]
mod tests;
