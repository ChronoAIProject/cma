//! Proof-bound recovery admission; no native side effects occur in a transaction.
use crate::store::transaction_terminal::GuardedTerminal as _;
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
use super::{
    agent_capacity_recovery::source::sandbox_uses_segments,
    agent_log_segments::transaction_scope::TransactionScope,
};
use transaction::ApplyError;

#[cfg(test)]
pub(in crate::store) mod mongo_integration_tests;
pub(in crate::store) mod transaction;

pub(super) async fn retained_snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<layer2_core::sandbox::RetainedRecoverySnapshot, SandboxStoreError> {
    layer2_core::diagnostic_context::DiagnosticContext::current()
        .with_worker("recovery_assessment")
        .scope(async {
            let mut session = store.client.start_session().await.map_err(unavailable)?;
            let result = async {
                if sandbox_uses_segments(store, id).await? {
                    let scope = TransactionScope::start(&mut session)
                        .await
                        .map_err(ApplyError::Driver)?;
                    transaction::load_retained_scoped(store, id, scope).await
                } else {
                    start_transaction(&mut session)
                        .await
                        .map_err(ApplyError::Driver)?;
                    transaction::load_retained(store, id, &mut session).await
                }
            }
            .await
            .map_err(ApplyError::domain);
            let _ = session.abort_guarded().await;
            result
        })
        .await
}

pub(super) async fn snapshot(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
    layer2_core::diagnostic_context::DiagnosticContext::current()
        .with_worker("recovery_assessment")
        .scope(async {
            let mut session = store.client.start_session().await.map_err(unavailable)?;
            let result = load_attempt(store, id, &mut session)
                .await
                .map(|(snapshot, _)| snapshot)
                .map_err(ApplyError::domain);
            let _ = session.abort_guarded().await;
            result
        })
        .await
}

pub(super) async fn admit(
    store: &MongoSandboxStore,
    request: SandboxUnpublishedRecoveryRequest,
) -> Result<Sandbox, SandboxStoreError> {
    request.proof.validate_for(&request.snapshot)?;
    let mut last_transient = None;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let receipt = match apply_attempt(store, &request, &mut session).await {
            Ok(receipt) => receipt,
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
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

async fn load_attempt(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut mongodb::ClientSession,
) -> Result<(SandboxUnpublishedRecoverySnapshot, u64), ApplyError> {
    if sandbox_uses_segments(store, id).await? {
        let scope = TransactionScope::start(session)
            .await
            .map_err(ApplyError::Driver)?;
        transaction::load_scoped(store, id, scope).await
    } else {
        start_transaction(session)
            .await
            .map_err(ApplyError::Driver)?;
        transaction::load(store, id, session).await
    }
}

async fn apply_attempt(
    store: &MongoSandboxStore,
    request: &SandboxUnpublishedRecoveryRequest,
    session: &mut mongodb::ClientSession,
) -> Result<transaction::Receipt, ApplyError> {
    if sandbox_uses_segments(store, &request.snapshot.sandbox.id).await? {
        let scope = TransactionScope::start(session)
            .await
            .map_err(ApplyError::Driver)?;
        transaction::apply_scoped(store, request, scope).await
    } else {
        start_transaction(session)
            .await
            .map_err(ApplyError::Driver)?;
        transaction::apply(store, request, session).await
    }
}
