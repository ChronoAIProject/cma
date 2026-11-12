//! Fence fresh recovery admission without turning historical replay into a write.
//! Provider work still requires independently qualified physical exclusion.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::SandboxId;
use mongodb::ClientSession;

use super::sandbox_mongo::MongoSandboxStore;
use super::sandbox_unpublished_recovery::transaction::ApplyError;

/// Call before recovery's history work and child/parent changes, in that same
/// transaction. Original proof, ownership, consent and source checks stay in
/// the existing recovery port. Exact immutable receipts return before this.
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
