//! Access workflow writes must join maintenance even when they only renew a
//! lease or settle a request. This does not authorize credential/provider work.
use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::store::{AgentProvenanceSnapshot, SandboxStoreError};

use super::*;

/// The caller immediately performs the existing real parent resource fence in
/// this transaction. Keep its serialization and source digest contribution.
pub(super) async fn check(
    store: &MongoSandboxStore,
    snapshot: &AgentProvenanceSnapshot,
    session: &mut ClientSession,
) -> TxResult<()> {
    crate::store::ledger_maintenance_writer_ports::parent_check(
        store,
        &snapshot.resource.lineage.sandbox.id,
        LedgerWriterClass::Credentials,
        session,
    )
    .await
    .map_err(map_error)
}

/// Resolve immutable custody from the stored workflow, not the proposed
/// replacement. Settlement may outlive the Agent, but it cannot borrow a
/// recreated parent or treat a missing parent as an unheld original home.
pub(super) async fn record(
    store: &MongoSandboxStore,
    candidate: &AgentAccessRecord,
    session: &mut ClientSession,
) -> TxResult<()> {
    let row = store
        .agent_access_requests
        .find_one(doc! {"_id":&candidate.id})
        .session(&mut *session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    let original = read(&row)?;
    if original.requester != candidate.requester || original.created_at != candidate.created_at {
        return Err(AgentAccessError::Conflict.into());
    }
    let identity = &original.requester;
    let row = store
        .sandboxes
        .find_one(doc! {"_id":identity.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AgentAccessError::NotFound)?;
    let parent =
        crate::store::sandboxes::read_sandbox(&row).map_err(|_| AgentAccessError::Unavailable)?;
    if parent.id != identity.sandbox_id
        || parent.id != identity.creation.sandbox_id
        || parent.owner != identity.owner
        || parent.created_at != identity.creation.sandbox_created_at
    {
        return Err(AgentAccessError::Conflict.into());
    }
    crate::store::ledger_maintenance_writer_ports::parent_write(
        store,
        &parent.id,
        LedgerWriterClass::Credentials,
        session,
    )
    .await
    .map_err(map_error)
}

fn map_error(error: crate::store::sandbox_events::TransactionEventError) -> Error {
    use crate::store::sandbox_events::TransactionEventError;
    match error {
        TransactionEventError::Driver(error) => Error::Driver(error),
        TransactionEventError::Domain(SandboxStoreError::NotFound) => {
            AgentAccessError::NotFound.into()
        }
        TransactionEventError::Domain(
            SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict,
        ) => AgentAccessError::Conflict.into(),
        TransactionEventError::Domain(_) => AgentAccessError::Unavailable.into(),
    }
}

#[cfg(test)]
#[path = "ledger_maintenance_access_tests.rs"]
mod tests;
