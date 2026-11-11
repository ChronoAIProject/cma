//! Fence an already-claimed worker's publication in its own transaction.
//! Shared publication entry points are wired by the integration owner.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Agent;
use mongodb::{bson::doc, ClientSession};

use super::agent_log_integrity::TransactionLogError as Error;
use super::ledger_maintenance::ApplyError;
use super::sandbox_mongo::MongoSandboxStore;

/// `current` is the actual Agent loaded in this same transaction, before any
/// prepared/history work. A prior worker claim cannot bypass a later hold.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<(), Error> {
    let row = store
        .sandboxes
        .find_one(doc! {"_id": current.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let parent = super::sandboxes::read_sandbox(&row)?;
    if parent.id != current.sandbox_id || parent.owner != current.owner {
        return Err(SandboxStoreError::Conflict.into());
    }
    super::ledger_maintenance_writer::fence_writer(
        store,
        &parent.id,
        LedgerWriterClass::AgentWorker,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => Error::Driver(error),
        ApplyError::Domain(error) => Error::Domain(match error {
            LedgerMaintenanceStoreError::WriterRefused(_)
            | LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::Conflict) => {
                SandboxStoreError::Conflict
            }
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                SandboxStoreError::NotFound
            }
            LedgerMaintenanceStoreError::Corrupt => {
                SandboxStoreError::Corrupt("ledger maintenance authority is inconsistent".into())
            }
            _ => SandboxStoreError::Unavailable("ledger maintenance authority unavailable".into()),
        }),
    })
}
