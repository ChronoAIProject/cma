//! Fence fresh recovery admission without turning historical replay into a write.
//! Provider work still requires independently qualified physical exclusion.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::SandboxId;
use mongodb::ClientSession;

use super::sandbox_mongo::MongoSandboxStore;
use super::sandbox_unpublished_recovery::transaction::ApplyError;

/// Refuse a held or inconsistent parent before history work without taking a
/// write lock across that scan. This check grants no effect authority; `fence`
/// remains mandatory in the same transaction before child/parent changes.
pub(super) async fn check(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    super::ledger_maintenance_writer_ports::parent_check(
        store,
        sandbox,
        LedgerWriterClass::Recovery,
        session,
    )
    .await?;
    Ok(())
}

/// The real parent write closes the race with maintenance admission. Keep it
/// after read-only source/history decisions, immediately before publication.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    super::ledger_maintenance_writer_ports::parent_write(
        store,
        sandbox,
        LedgerWriterClass::Recovery,
        session,
    )
    .await?;
    Ok(())
}
