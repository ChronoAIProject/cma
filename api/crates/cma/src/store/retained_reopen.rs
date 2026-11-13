//! Atomic, owner-consented reopening on an already restored retained runtime.
use layer2_core::{
    sandbox::{
        RetainedReopenAccepted, RetainedReopenReceipt, RetainedReopenRequest,
        RetainedReopenSnapshot, SandboxStoreError,
    },
    SandboxId,
};
use mongodb::{bson::doc, options::ReadConcern};

use super::agent_log_segments::transaction_scope::TransactionScope;
use super::sandbox_mongo::{
    commit_transaction, is_transient, wait_for_reconciliation, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};
use super::sandbox_unpublished_recovery::transaction::ApplyError;
use super::sandbox_unpublished_recovery::transaction::NativeFacts;

mod load;
#[cfg(test)]
pub(in crate::store) mod mongo_fixtures;
#[cfg(test)]
mod mongo_tests;
mod transaction;
const FIELD: &str = "retained_reopen_receipt_v1";

pub(super) async fn snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<RetainedReopenSnapshot, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(unavailable)?;
    let facts = NativeFacts::capture_reopen(store, id, &mut scope).await;
    let session = scope.release();
    let result = match facts {
        Ok(facts) => load::snapshot_with_sources(store, id, session, Some(&facts)).await,
        Err(error) => Err(error),
    }
    .map(|(snapshot, _)| snapshot)
    .map_err(ApplyError::domain);
    let _ = session.abort_transaction().await;
    result
}

pub(super) async fn receipt(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Option<RetainedReopenReceipt>, SandboxStoreError> {
    let row = store
        .sandboxes
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::majority())
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    transaction::read_receipt(&row)
}

pub(super) async fn admit(
    store: &MongoSandboxStore,
    request: RetainedReopenRequest,
) -> Result<RetainedReopenAccepted, SandboxStoreError> {
    request.proof.validate(&request.snapshot)?;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let applied = match transaction::apply_scoped(store, &request, scope).await {
            Ok(applied) => applied,
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error.domain());
            }
        };
        // An exact replay needs no write or event and was read at majority.
        let transaction::Applied::Commit { receipt, event } = applied else {
            let transaction::Applied::Replay(accepted) = applied else {
                unreachable!()
            };
            let _ = session.abort_transaction().await;
            return Ok(accepted);
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.accepted),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(error) => {
                let time = session.operation_time().ok_or_else(|| {
                    unavailable(format!("recovery commit time unavailable: {error}"))
                })?;
                let store = store.clone();
                return wait_for_reconciliation("retained reopening commit", async move {
                    transaction::reconcile(&store, &receipt, &event, time).await?;
                    Ok(receipt.accepted)
                })
                .await
                .map_err(unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(unavailable(
        "retained reopening transaction retries exhausted",
    ))
}

fn domain(message: &'static str) -> ApplyError {
    ApplyError::Domain(SandboxStoreError::InvalidTransaction(message))
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
