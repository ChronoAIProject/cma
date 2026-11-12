//! Maintenance admission for runtime output, including checkpoint-only writes.
//! Exact immutable receipt reads remain outside these mutation guards.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::Agent;
use mongodb::ClientSession;

use super::agent_log_integrity::TransactionLogError;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) async fn recorder(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    super::ledger_maintenance_publication::fence_as(
        store,
        current,
        LedgerWriterClass::Recorder,
        session,
    )
    .await
}

/// Checkpoint carry receives the validated replacement Agent after its CAS in
/// this transaction. Quiet/reanchor writes receive the actual loaded Agent.
pub(super) async fn checkpoint(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    super::ledger_maintenance_publication::fence_as(
        store,
        current,
        LedgerWriterClass::Checkpoint,
        session,
    )
    .await
}

/// Reopen receipt replay also writes an authority fence. Guard it before that
/// effect, while leaving ordinary immutable checkpoint replay read-only.
pub(super) async fn reopen(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    super::ledger_maintenance_publication::fence_as(
        store,
        current,
        LedgerWriterClass::Output,
        session,
    )
    .await
}
