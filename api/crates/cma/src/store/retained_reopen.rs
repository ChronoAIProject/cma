//! Atomic, owner-consented reopening on an already restored retained runtime.
use layer2_core::{
    sandbox::{
        RetainedReopenAccepted, RetainedReopenReceipt, RetainedReopenRequest,
        RetainedReopenSnapshot, SandboxStoreError,
    },
    SandboxId,
};
use mongodb::{bson::doc, options::ReadConcern};

use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use super::sandbox_unpublished_recovery::transaction::ApplyError;

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
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = load::snapshot(store, id, &mut session)
        .await
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
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        let applied = match transaction::apply(store, &request, &mut session).await {
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
