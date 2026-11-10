//! Proof-bound recovery admission; no native side effects occur in a transaction.
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Sandbox, SandboxUnpublishedRecoveryRequest, SandboxUnpublishedRecoverySnapshot,
};
use layer2_core::SandboxId;
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};

use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use transaction::ApplyError;

#[cfg(test)]
mod mongo_integration_tests;
mod transaction;

pub(super) async fn retained_snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = transaction::load_retained(store, id, &mut session)
        .await
        .map_err(ApplyError::domain);
    let _ = session.abort_transaction().await;
    result
}

pub(super) async fn snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = transaction::load(store, id, &mut session)
        .await
        .map(|(snapshot, _)| snapshot)
        .map_err(ApplyError::domain);
    let _ = session.abort_transaction().await;
    result
}

pub(super) async fn admit(
    store: &MongoSandboxStore,
    request: SandboxUnpublishedRecoveryRequest,
) -> Result<Sandbox, SandboxStoreError> {
    request.proof.validate_for(&request.snapshot)?;
    let mut last_transient = None;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(unavailable(error));
        }
        let receipt = match transaction::apply(store, &request, &mut session).await {
            Ok(receipt) => receipt,
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error.domain());
            }
        };
        let mut commit = Observation::new(Stage::Commit);
        commit.pending(Reason::UnknownCommit);
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                commit.finish(Reason::Success);
                return Ok(receipt.sandbox);
            }
            CommitOutcome::Retry(error) => {
                commit.finish(Reason::Unavailable);
                last_transient = Some(error.to_string());
            }
            CommitOutcome::Unknown(error) => {
                commit.finish(Reason::UnknownCommit);
                drop(commit);
                let time = session.operation_time().ok_or_else(|| {
                    unavailable(format!(
                        "unpublished runtime recovery commit has no operation time: {error}"
                    ))
                })?;
                let store = store.clone();
                return wait_for_reconciliation(
                    "unpublished runtime recovery commit",
                    async move {
                        let mut reconciliation = Observation::new(Stage::ReconcileCommit);
                        let result = transaction::reconcile(&store, &receipt, time).await;
                        reconciliation.finish(match &result {
                            Ok(true) => Reason::Success,
                            _ => Reason::UnresolvedCommit,
                        });
                        if result? {
                            Ok(receipt.sandbox)
                        } else {
                            Err(unavailable(
                                "unpublished runtime recovery commit is unresolved",
                            ))
                        }
                    },
                )
                .await
                .map_err(unavailable)?;
            }
            CommitOutcome::Failed(error) => {
                commit.finish(Reason::Unavailable);
                return Err(unavailable(error));
            }
        }
        tokio::task::yield_now().await;
    }
    Err(unavailable(format!(
        "unpublished runtime recovery exhausted retries: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
