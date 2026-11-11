//! Pre-exclusion persistence is a separate port from V1 execution maintenance.

use super::ledger_maintenance::LedgerMaintenanceStoreError;
use crate::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1;
use crate::sandbox::ledger_maintenance_pre_exclusion::LedgerPreExclusionStateV1;
use crate::sandbox::ledger_maintenance_pre_exclusion_progress::{
    LedgerPreExclusionAdmission, LedgerPreExclusionPolicy, LedgerPreExclusionProgress,
};
use crate::{Caller, SandboxId};
use async_trait::async_trait;

#[derive(Clone)]
pub struct LedgerPreExclusionInspection {
    pub source: LedgerMaintenanceSnapshotV1,
    pub state: Option<LedgerPreExclusionStateV1>,
}

#[async_trait]
pub trait LedgerPreExclusionStore: Send + Sync {
    async fn inspect_pre_exclusion(
        &self,
        caller: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerPreExclusionInspection, LedgerMaintenanceStoreError>;

    async fn admit_pre_exclusion(
        &self,
        policy: LedgerPreExclusionPolicy,
        request: LedgerPreExclusionAdmission,
    ) -> Result<LedgerPreExclusionStateV1, LedgerMaintenanceStoreError>;

    async fn progress_pre_exclusion(
        &self,
        policy: LedgerPreExclusionPolicy,
        request: LedgerPreExclusionProgress,
    ) -> Result<LedgerPreExclusionStateV1, LedgerMaintenanceStoreError>;
}
