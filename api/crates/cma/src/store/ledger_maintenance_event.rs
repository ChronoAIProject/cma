//! The standalone event port owns a separate transaction from aggregate saves.
//! Internal event appends keep their caller's guard; reconciliation stays a read.
use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;

use super::*;

pub(super) async fn append(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    sandbox_id: &SandboxId,
    appended_at: String,
    kind: SandboxEventKind,
) -> Result<SandboxEvent, TransactionEventError> {
    crate::store::ledger_maintenance_writer_ports::parent_write(
        store,
        sandbox_id,
        LedgerWriterClass::Administration,
        session,
    )
    .await?;
    append_in_transaction(store, session, sandbox_id, appended_at, kind).await
}

#[cfg(test)]
#[path = "ledger_maintenance_event_tests.rs"]
mod tests;
