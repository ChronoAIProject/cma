//! Durable maintenance admission port. Inspect never admits or renews a hold.

use async_trait::async_trait;

use crate::sandbox::ledger_maintenance_admission::{
    LedgerMaintenanceAdmissionPolicy, LedgerMaintenanceAdmissionRefusal, LedgerMaintenanceLeaseV1,
    LedgerMaintenanceStateV1,
};
use crate::sandbox::ledger_maintenance_plan::{
    LedgerMaintenancePlanArtifactV1, LedgerMaintenanceSnapshotV1,
};
use crate::{Caller, SandboxId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerMaintenanceStoreError {
    #[error(transparent)]
    Refused(#[from] LedgerMaintenanceAdmissionRefusal),
    #[error(transparent)]
    WriterRefused(#[from] crate::sandbox::ledger_maintenance::LedgerMaintenanceRefusal),
    #[error("ledger maintenance authority is unavailable")]
    Unavailable,
    #[error("ledger maintenance authority is inconsistent")]
    Corrupt,
    #[error("ledger maintenance commit remains unresolved; retain the original operation")]
    PendingCommit,
}

#[derive(Clone)]
pub struct LedgerMaintenanceAdmission {
    pub caller: Caller,
    pub accepted: LedgerMaintenancePlanArtifactV1,
    pub lease: LedgerMaintenanceLeaseV1,
    pub now: String,
}

#[derive(Clone)]
pub struct LedgerMaintenanceTakeover {
    pub caller: Caller,
    pub expected: LedgerMaintenanceStateV1,
    pub expected_source: LedgerMaintenanceSnapshotV1,
    pub lease: LedgerMaintenanceLeaseV1,
    pub now: String,
}

/// The returned records are private authenticated authority, never public logs.
#[derive(Clone)]
pub struct LedgerMaintenanceInspection {
    pub source: LedgerMaintenanceSnapshotV1,
    pub state: Option<LedgerMaintenanceStateV1>,
}

#[async_trait]
pub trait LedgerMaintenanceStore: Send + Sync {
    async fn inspect_ledger_maintenance(
        &self,
        caller: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerMaintenanceInspection, LedgerMaintenanceStoreError>;

    async fn admit_ledger_maintenance(
        &self,
        policy: LedgerMaintenanceAdmissionPolicy,
        request: LedgerMaintenanceAdmission,
    ) -> Result<LedgerMaintenanceStateV1, LedgerMaintenanceStoreError>;

    async fn take_over_ledger_maintenance(
        &self,
        policy: LedgerMaintenanceAdmissionPolicy,
        request: LedgerMaintenanceTakeover,
    ) -> Result<LedgerMaintenanceStateV1, LedgerMaintenanceStoreError>;
}
