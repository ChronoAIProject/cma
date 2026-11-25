//! Atomic metering of runtime/home edges that precede a status event.

use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, unsigned, CommitOutcome,
    TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::released_runtime::ReleasedLane;
use layer3_adapter::ReleasedRuntime;

pub(in crate::store) async fn save(
    store: &MongoSandboxStore,
    expected: u64,
    sandbox: &Sandbox,
    acquisition: Option<&Document>,
    release: Option<(&ReleasedRuntime, ReleasedLane)>,
) -> Result<(), SandboxStoreError> {
    let at = sandbox.updated_at.clone();
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        match apply(
            store,
            &mut session,
            expected,
            sandbox,
            &at,
            acquisition,
            release,
        )
        .await
        {
            Ok(()) => {}
            Err(TransactionEventError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(TransactionEventError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(unavailable(error));
            }
            Err(TransactionEventError::Domain(error)) => {
                let _ = session.abort_guarded().await;
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
    acquisition: Option<&Document>,
    release: Option<(&ReleasedRuntime, ReleasedLane)>,
) -> Result<(), TransactionEventError> {
    use TransactionEventError::{Domain, Driver};
    crate::store::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => Domain(error),
            crate::store::work_publication::FenceError::Driver(error) => Driver(error),
        })?;
    if acquisition.is_some() && sandbox.work.authority_protocol.is_some() {
        crate::store::work_schema::floor::retain(store, session)
            .await
            .map_err(Driver)?;
    }
    let current = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(Driver)?
        .ok_or(Domain(SandboxStoreError::NotFound))?;
    let parent = crate::store::sandboxes::read_sandbox(&current).map_err(Domain)?;
    crate::store::native_retirement_action::preserve_custody(&current, &parent, sandbox)
        .map_err(Domain)?;
    parent
        .validate_retirement_archive_successor(sandbox)
        .map_err(|_| Domain(SandboxStoreError::Conflict))?;
    if acquisition.is_some() {
        crate::store::sandbox_credentials::issuance::legacy::preserve(
            store, &parent, sandbox, session,
        )
        .await
        .map_err(Driver)?;
    }
    if let Some((release, lane)) = release {
        store
            .fence_released_admission(release, lane, session)
            .await
            .map_err(Domain)?;
    }
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
    let mut filter = doc! {
        "_id": sandbox.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": expected, "origin_type": origin_type, "origin_launch_id": launch_id,
        "origin_operation_id": operation_id,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_retirement_finalization": { "$exists": false },
    };
    if let Some(acquisition) = acquisition {
        // The authority predicate and metering remain in this exact write's
        // transaction. A pre-read cannot authorize delayed claim delivery.
        filter = doc! {"$and":[filter, acquisition.clone()]};
    }
    let filter =
        crate::store::work_publication::sandbox(filter, sandbox.id.as_str()).map_err(Domain)?;
    let mut replacement =
        crate::store::sandboxes::sandbox_replacement_fields(sandbox).map_err(Domain)?;
    if acquisition.is_some() {
        crate::store::released_runtimes::pin_first_dispatch(
            store,
            &current,
            &parent,
            &mut replacement,
        )
        .map_err(Domain)?;
    } else {
        crate::store::released_runtimes::pin_replacement_dispatch(
            store,
            &parent,
            sandbox,
            &mut replacement,
        )
        .map_err(Domain)?;
    }
    let updated = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(filter),
            if acquisition.is_some() {
                crate::store::work_lease_update::acquisition(replacement)
            } else {
                crate::store::work_lease_update::replacement(replacement)
            },
        )
        .session(&mut *session)
        .await
        .map_err(Driver)?;
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
        crate::store::sandbox_usage::UsageObservation::Lifecycle,
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
        crate::store::sandbox_usage::UsageObservation::Lifecycle,
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
