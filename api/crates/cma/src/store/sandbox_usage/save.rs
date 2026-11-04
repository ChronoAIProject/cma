//! Atomic metering of runtime/home edges that precede a status event.

use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, unsigned, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};

pub(in crate::store) async fn save(
    store: &MongoSandboxStore,
    expected: u64,
    sandbox: &Sandbox,
) -> Result<(), SandboxStoreError> {
    let at = sandbox.updated_at.clone();
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        match apply(store, &mut session, expected, sandbox, &at).await {
            Ok(()) => {}
            Err(TransactionEventError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(TransactionEventError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(unavailable(error));
            }
            Err(TransactionEventError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            // An ambiguous commit cannot trigger an external effect. The next
            // store read reconciles the Sandbox revision before another save.
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(unavailable(error))
            }
        }
    }
    Err(SandboxStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    expected: u64,
    sandbox: &Sandbox,
    at: &str,
) -> Result<(), TransactionEventError> {
    use TransactionEventError::{Domain, Driver};
    let current = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(Driver)?
        .ok_or(Domain(SandboxStoreError::NotFound))?;
    let previous_revision = expected;
    let expected = signed(expected, "sandbox revision")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error)))?;
    if current.get_i64("revision").ok() != Some(expected) {
        return Err(Domain(SandboxStoreError::Conflict));
    }
    let seq = current
        .get_i64("last_seq")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    let seq = unsigned(seq, "sandbox sequence")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error)))?;
    let (origin_type, launch_id, operation_id) =
        crate::store::sandboxes::sandbox_origin_fields(sandbox).map_err(Domain)?;
    let updated = store.sandboxes.update_one(doc! {
        "_id": sandbox.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": expected, "origin_type": origin_type, "origin_launch_id": launch_id,
        "origin_operation_id": operation_id,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
    }, doc! { "$set": crate::store::sandboxes::sandbox_replacement_fields(sandbox).map_err(Domain)? })
        .session(&mut *session).await.map_err(Driver)?;
    if updated.matched_count != 1 {
        return Err(Domain(SandboxStoreError::Conflict));
    }
    super::record(
        store,
        session,
        sandbox,
        seq,
        at,
        (seq, previous_revision),
        false,
    )
    .await
}

/// Every aggregate revision participates, including lease and credential saves.
/// Call after a successful revision CAS and before committing the same session.
pub(in crate::store) async fn record_saved(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    sandbox: &Sandbox,
    expected_revision: u64,
) -> Result<(), TransactionEventError> {
    use TransactionEventError::{Domain, Driver};
    // A fence-only write has no new observation time. In particular it must not
    // initialize an untracked legacy Sandbox at its historical updated_at.
    if sandbox.revision == expected_revision {
        return Ok(());
    }
    let document = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(Driver)?
        .ok_or(Domain(SandboxStoreError::NotFound))?;
    let seq = document
        .get_i64("last_seq")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    let seq = unsigned(seq, "sandbox sequence")
        .map_err(|error| Domain(SandboxStoreError::Corrupt(error)))?;
    super::record(
        store,
        session,
        sandbox,
        seq,
        &sandbox.updated_at,
        (seq, expected_revision),
        false,
    )
    .await
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    if is_duplicate_key(&error) {
        SandboxStoreError::Duplicate
    } else {
        SandboxStoreError::Unavailable(error.to_string())
    }
}
