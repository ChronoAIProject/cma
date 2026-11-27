//! Complete Mongo runtime authority and atomic recovery from runtime loss.

use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{
    validate_sandbox_runtime_loss_request, Page, PageRequest, SandboxReconciliationStore,
    SandboxRuntimeLossOutcome, SandboxRuntimeLossRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Sandbox;
use mongodb::bson::doc;
use mongodb::error::{ErrorKind, WriteFailure};
use sha2::{Digest, Sha256};

use super::native_retirement_action::RetirementAction;
use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;
use super::{
    agent_capacity_recovery::source::sandbox_uses_segments,
    agent_log_segments::transaction_scope::TransactionScope,
};
use transaction::{apply, ApplyError, RecoveryApplication};

mod agents;
pub(super) use agents::settle_reopen_agents;
#[cfg(test)]
mod mongo_integration_tests;
mod reconciliation;
mod telemetry;
mod transaction;

#[async_trait::async_trait]
impl SandboxReconciliationStore for MongoSandboxStore {
    async fn unowned_home_reservation(
        &self,
        id: &layer2_core::SandboxId,
    ) -> Result<Option<layer2_core::sandbox::SandboxHome>, SandboxStoreError> {
        self.native_prepared_home_reservation(id).await
    }

    async fn unowned_home_authority(
        &self,
        request: &layer3_adapter::HomeVolumeRequest,
    ) -> Result<bool, SandboxStoreError> {
        if self.native_prepared_home_authority(request).await? {
            return Ok(true);
        }
        // A retained Channel home stays held even when its canonical home or
        // root row was lost. Raw marker presence is a hold, not ownership proof.
        let sandbox = self
            .sandboxes
            .find_one(doc! { "_id": &request.sandbox_id })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if super::channel_storage_retention::required(
            &self.client.database(&self.sandboxes.namespace().db),
            &request.sandbox_id,
            sandbox.as_ref(),
            None,
        )
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            return Ok(true);
        }
        let Some(reservation) = self
            .warm_sandbox(&layer2_core::SandboxId(request.sandbox_id.clone()))
            .await?
        else {
            return Ok(false);
        };
        reservation.validate()?;
        Ok(reservation.state.owns_runtime()
            && reservation
                .home
                .as_ref()
                .is_some_and(|home| !home.released && home.request == *request))
    }

    async fn unowned_runtime_authority(
        &self,
        id: &layer2_core::SandboxId,
        generation: u64,
        runtime_id: &str,
    ) -> Result<bool, SandboxStoreError> {
        if self
            .native_prepared_runtime_authority(id, generation, runtime_id)
            .await?
        {
            return Ok(true);
        }
        let Some(reservation) = self.warm_sandbox(id).await? else {
            return Ok(false);
        };
        Ok(reservation.state.owns_runtime()
            && generation == 1
            && reservation
                .runtime
                .as_ref()
                .is_none_or(|binding| binding.runtime_id == runtime_id))
    }

    async fn sandbox_runtime_authority(
        &self,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! {};
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandboxes
            .find(crate::store::work_schema::query(filter))
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_sandbox(&document)?);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("bounded runtime authority page has a last item")
                .id
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn recover_runtime_loss(
        &self,
        request: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        validate_sandbox_runtime_loss_request(&request)?;
        recover(self, request, None).await
    }
}

async fn recover(
    store: &MongoSandboxStore,
    request: SandboxRuntimeLossRequest,
    retirement: Option<&RetirementAction>,
) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let receipt = match apply_attempt(store, &request, retirement, &mut session).await {
            Ok(RecoveryApplication::Complete(outcome)) => {
                let _ = session.abort_guarded().await;
                return Ok(outcome);
            }
            Ok(RecoveryApplication::Persist(receipt)) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                let write_conflict = match error.kind.as_ref() {
                    ErrorKind::Command(error) => error.code == 112,
                    ErrorKind::Write(WriteFailure::WriteError(error)) => error.code == 112,
                    _ => false,
                };
                if write_conflict && retirement.is_none() {
                    tracing::debug!(
                        target: "cma::agent_manager::writer_publication",
                        parent: None,
                        sandbox_sha256 = %format_args!("{:x}", Sha256::digest(request.sandbox_id.as_str().as_bytes())),
                        operation = "runtime_loss",
                        failure = "known_aborted_cas",
                        "Runtime loss transaction aborted before rebuilding recovery"
                    );
                }
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        let commit = {
            let mut phase = telemetry::Phase::new("commit");
            let result = commit_transaction(&mut session).await;
            phase.finish(matches!(&result, CommitOutcome::Committed));
            result
        };
        match commit {
            CommitOutcome::Committed => return Ok(receipt.outcome),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "runtime-loss commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("runtime-loss recovery commit", async move {
                    if reconciliation::reconcile(&store, &receipt, snapshot_time).await? {
                        Ok(receipt.outcome)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "runtime-loss recovery commit remained unknown: {unresolved}"
                        )))
                    }
                })
                .await
                .map_err(|deadline| {
                    SandboxStoreError::Unavailable(format!(
                        "{deadline}; ambiguous commit: {commit_error}"
                    ))
                })?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()))
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "runtime-loss recovery exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

/// The format hint selects a transaction entry, never proof or adoption.
/// A conversion racing the legacy entry is rejected by its source validator.
async fn apply_attempt(
    store: &MongoSandboxStore,
    request: &SandboxRuntimeLossRequest,
    retirement: Option<&RetirementAction>,
    session: &mut mongodb::ClientSession,
) -> Result<RecoveryApplication, ApplyError> {
    if sandbox_uses_segments(store, &request.sandbox_id).await? {
        let scope = TransactionScope::start(session)
            .await
            .map_err(ApplyError::Driver)?;
        transaction::apply_scoped(store, request, retirement, scope).await
    } else {
        start_transaction(session)
            .await
            .map_err(ApplyError::Driver)?;
        apply(store, request, retirement, session).await
    }
}

impl MongoSandboxStore {
    pub(crate) async fn settle_retirement_action(
        &self,
        request: SandboxRuntimeLossRequest,
        action: &RetirementAction,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        validate_sandbox_runtime_loss_request(&request)?;
        recover(self, request, Some(action)).await
    }

    pub(crate) fn with_runtime_loss_bounded_proof(mut self, enabled: bool) -> Self {
        self.runtime_loss_bounded_proof_enabled = enabled;
        self
    }
}
