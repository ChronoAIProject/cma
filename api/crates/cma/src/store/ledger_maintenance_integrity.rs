//! Integrity metadata is a writer, including quarantine after an audit abort.
//! Shared preparation/quarantine call sites remain integration-owned.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Agent;
use mongodb::{bson::Document, ClientSession};

use super::agent_log_integrity::TransactionLogError;
use super::ledger_maintenance::ApplyError;
use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};

/// Call before an audit that may write metadata. An audit which aborts and then
/// quarantines must acquire a fresh fence in its separate quarantine transaction.
pub(super) async fn fence_metadata(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    super::ledger_maintenance_writer::fence_writer(
        store,
        &agent.sandbox_id,
        LedgerWriterClass::Reconciliation,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => TransactionLogError::Driver(error),
        ApplyError::Domain(error) => TransactionLogError::Domain(match error {
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

/// Preserve the existing quarantine payload and immutable-prefix predicate.
/// The parent barrier and Agent update now commit together. Unknown commit never
/// starts a fresh transaction or reports confirmed success.
pub(super) async fn quarantine_update(
    store: &MongoSandboxStore,
    observed: &Agent,
    filter: Document,
    update: Document,
) -> Result<(), TransactionLogError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(TransactionLogError::Driver)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(TransactionLogError::Driver(error));
        }
        match apply_quarantine(store, observed, &filter, &update, &mut session).await {
            Ok(()) => {}
            Err(TransactionLogError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => {
                tokio::task::yield_now().await;
            }
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(TransactionLogError::Driver(error));
            }
        }
    }
    Err(SandboxStoreError::Unavailable("quarantine transaction retries exhausted".into()).into())
}

pub(super) async fn apply_quarantine(
    store: &MongoSandboxStore,
    observed: &Agent,
    filter: &Document,
    update: &Document,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let current = store
        .agents
        .find_one(mongodb::bson::doc! {"_id": observed.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let current = super::agents::read_agent(&current)?;
    // A newer revision may retain the disproved prefix. A replaced or moved
    // history must not receive a delayed quarantine from the prior incarnation.
    if current.id != observed.id
        || current.owner != observed.owner
        || current.sandbox_id != observed.sandbox_id
        || current.workspace_id != observed.workspace_id
        || current.worktree_id != observed.worktree_id
        || current.creation_key != observed.creation_key
        || current.creation_fingerprint != observed.creation_fingerprint
        || current.created_at != observed.created_at
        || current.conversation.last_seq < observed.conversation.last_seq
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent history changed before quarantine could be persisted".into(),
        )
        .into());
    }
    fence_metadata(store, &current, session).await?;
    let result = store
        .agents
        .update_one(filter.clone(), update.clone())
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Corrupt(
            "Agent history changed before quarantine could be persisted".into(),
        )
        .into());
    }
    Ok(())
}
