//! Fence receipt-only Sandbox controls as well as lifecycle transitions.
//! The existing control transaction still owns authorization and queue checks.

use layer2_core::sandbox::control::{SandboxControlError, SandboxControlRecord};
use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use mongodb::ClientSession;

use super::ledger_maintenance::ApplyError;
use super::sandbox_controls::ApplyError as ControlError;
use super::sandbox_mongo::MongoSandboxStore;

/// Resolve the original target in the same transaction, including rejection of
/// an already queued command. A historical receipt cannot fence a recreated
/// Sandbox, and an orphan is not evidence that its former home is unheld.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    record: &SandboxControlRecord,
    session: &mut ClientSession,
) -> Result<(), ControlError> {
    record.validate()?;
    let target = record.envelope.target();
    let snapshot = super::resource_access::read_execution_snapshot(store, &target, session).await?;
    let scope = snapshot
        .scope(&target)
        .map_err(|_| SandboxControlError::Corrupt)?;
    if snapshot.lineage.sandbox.id != record.envelope.sandbox_id
        || snapshot.lineage.sandbox.created_at != record.envelope.sandbox_created_at
        || scope.owner != record.envelope.owner
    {
        return Err(SandboxControlError::Conflict.into());
    }
    super::ledger_maintenance_writer::fence_writer(
        store,
        &snapshot.lineage.sandbox.id,
        LedgerWriterClass::OwnerControl,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => ControlError::Driver(error),
        ApplyError::Domain(error) => ControlError::Domain(match error {
            LedgerMaintenanceStoreError::WriterRefused(_)
            | LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::Conflict) => {
                SandboxControlError::Conflict
            }
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                SandboxControlError::NotFound
            }
            LedgerMaintenanceStoreError::Corrupt => SandboxControlError::Corrupt,
            _ => SandboxControlError::Unavailable,
        }),
    })
}
