//! Evidence-backed Mongo admission of automatic Sandbox maintenance.

use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::ids::SandboxId;
use layer2_core::now_rfc3339;
use layer2_core::sandbox::store::{
    validate_image_rotation_request, validate_inactivity_pause_request, InactivityPausePreview,
    Page, PageRequest, SandboxImageRotationRequest, SandboxInactivityPauseRequest,
    SandboxMaintenanceOutcome, SandboxMaintenanceStore, SandboxStore, SandboxStoreError,
    MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{Sandbox, SandboxStatus};
use mongodb::bson::doc;

use super::agent_log_integrity::IntegrityFailurePolicy;
use super::sandbox_mongo::{
    commit_transaction, is_transient, wait_for_reconciliation, CommitOutcome, MongoSandboxStore,
};
use super::sandboxes::read_sandbox;
use transaction::{attempt as apply_attempt, reconcile, ApplyError, MaintenanceRequest};

mod consented;
mod custom_runtime;
mod desktop_custody;
mod evidence;
#[cfg(test)]
pub(crate) mod mongo_integration_tests;
mod policy_fence;
mod retained_availability;
pub(crate) mod retained_transition;
mod retry;
mod reviewed_retirement;
pub(super) mod runtime_activity;
mod transaction;

#[async_trait::async_trait]
impl SandboxMaintenanceStore for MongoSandboxStore {
    async fn admit_custom_runtime_migration(
        &self,
        admission: layer2_core::sandbox::store::SandboxCustomRuntimeMigrationAdmission,
    ) -> Result<Sandbox, SandboxStoreError> {
        custom_runtime::admit(self, admission).await
    }

    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "status": { "$in": ["active", "paused"] } };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandboxes
            .find(super::sandbox_quarantine::exclude(
                crate::store::work_schema::query(filter),
            ))
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
            items.push(read_sandbox(&document)?.id);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("bounded maintenance page has a last item")
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_inactivity_pause_request(&request)?;
        let source = match request.cause {
            layer2_core::sandbox::store::InactivityPauseCause::Inactivity => "automatic",
            layer2_core::sandbox::store::InactivityPauseCause::OperatorInactivity => "operator",
            layer2_core::sandbox::store::InactivityPauseCause::PlatformSpendStop => "spend_stop",
        };
        let prepare = request.auto_pause_enabled
            && request.expected_policy_revision.is_none()
            && request.cause == layer2_core::sandbox::store::InactivityPauseCause::Inactivity;
        let result = schedule_receipt(self, MaintenanceRequest::Inactivity(request)).await;
        if prepare {
            if let Some(agent) = result
                .as_ref()
                .ok()
                .and_then(|receipt| receipt.next_legacy_proof.as_deref())
            {
                evidence::certified_legacy::prepare_next(self, agent).await;
            }
        }
        let result = result.map(|receipt| receipt.outcome);
        let outcome = match &result {
            Ok(SandboxMaintenanceOutcome::Scheduled) => "scheduled",
            Ok(SandboxMaintenanceOutcome::AlreadyScheduled) => "already_scheduled",
            Ok(SandboxMaintenanceOutcome::NotEligible) => "not_eligible",
            Err(_) => "unavailable",
        };
        metrics::counter!("cma_auto_pause_decisions_total", "source" => source, "outcome" => outcome).increment(1);
        result
    }

    async fn preview_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        Ok(self
            .preview_inactivity_pause_with_reason(request)
            .await?
            .outcome)
    }

    async fn preview_inactivity_pause_with_reason(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<InactivityPausePreview, SandboxStoreError> {
        validate_inactivity_pause_request(&request)?;
        let receipt = preview(self, MaintenanceRequest::Inactivity(request)).await?;
        Ok(InactivityPausePreview {
            outcome: receipt.outcome,
            refusal: receipt.refusal,
        })
    }

    async fn set_sandbox_always_on(
        &self,
        sandbox_id: SandboxId,
        expected_revision: u64,
        always_on: bool,
    ) -> Result<Sandbox, SandboxStoreError> {
        let current = self
            .get_sandbox(&sandbox_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if matches!(
            current.status,
            SandboxStatus::Retiring | SandboxStatus::Retired
        ) || current.revision != expected_revision
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut next = current;
        next.profile.always_on = Some(always_on);
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next.updated_at = now_rfc3339();
        next.validate()
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        self.save_sandbox(expected_revision, next.clone()).await?;
        Ok(next)
    }

    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_image_rotation_request(&request)?;
        custom_runtime::schedule_rotation(self, request).await
    }
}

async fn schedule(
    store: &MongoSandboxStore,
    request: MaintenanceRequest,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
    Ok(schedule_receipt(store, request).await?.outcome)
}

async fn schedule_receipt(
    store: &MongoSandboxStore,
    request: MaintenanceRequest,
) -> Result<transaction::MaintenanceReceipt, SandboxStoreError> {
    if request.requires_desktop_guard() {
        let id = request.sandbox_id().clone();
        let store = store.clone();
        return desktop_custody::run(&id, move |custody| async move {
            schedule_owned(&store, request, Some(custody)).await
        })
        .await;
    }
    schedule_owned(store, request, None).await
}

async fn schedule_owned(
    store: &MongoSandboxStore,
    request: MaintenanceRequest,
    mut custody: Option<desktop_custody::Custody>,
) -> Result<transaction::MaintenanceReceipt, SandboxStoreError> {
    let mut last_transient = None;
    let mut retries = retry::Retries::new();
    while retries.next().await {
        let mut session = match store.client.start_session().causal_consistency(true).await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        if let Some(basis) = request.basis() {
            if let layer2_core::sandbox::store::RuntimeMigrationClearance::Observed { proof } =
                &basis.clearance
            {
                let watermark = proof.final_observation.watermark;
                session.advance_operation_time(mongodb::bson::Timestamp {
                    time: watermark.seconds,
                    increment: watermark.increment,
                });
            }
        }
        let receipt = match apply_attempt(
            store,
            &request,
            &mut session,
            IntegrityFailurePolicy::Quarantine,
        )
        .await
        {
            Ok(receipt) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        if receipt.event.is_none() {
            let _ = session.abort_guarded().await;
            return Ok(receipt);
        }
        if let Some(basis) = request.basis() {
            if let Err(error) =
                basis.validate_at(layer2_core::sandbox::store::runtime_migration_now_ms()?)
            {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
        }
        if let Some(custody) = &mut custody {
            custody.commit_started();
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                if let Some(custody) = &mut custody {
                    custody.confirmed();
                }
                return Ok(receipt);
            }
            CommitOutcome::Retry(error) => {
                if let Some(custody) = &mut custody {
                    custody.confirmed();
                }
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
            }
            CommitOutcome::Unknown(error) => {
                if let Some(custody) = &mut custody {
                    // Already owned before effects. Do not acquire another
                    // reconciliation slot or move finality outside that owner.
                    custody
                        .reconcile(session.operation_time(), |snapshot_time| {
                            reconcile(store, &receipt, snapshot_time)
                        })
                        .await?;
                    return Ok(receipt);
                }
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "sandbox maintenance commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("sandbox maintenance commit", async move {
                    if reconcile(&store, &receipt, snapshot_time).await? {
                        Ok(receipt)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "sandbox maintenance commit remained unknown: {unresolved}"
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
                if let Some(custody) = &mut custody {
                    custody.confirmed();
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "sandbox maintenance exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn preview(
    store: &MongoSandboxStore,
    request: MaintenanceRequest,
) -> Result<transaction::MaintenanceReceipt, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    // Preview owns the abort; integrity failures must not persist quarantine outside it.
    let result = apply_attempt(
        store,
        &request,
        &mut session,
        IntegrityFailurePolicy::ReportOnly,
    )
    .await;
    let abort = session.abort_guarded().await;
    if result.is_ok() {
        abort.map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    }
    match result {
        Ok(receipt) => Ok(receipt),
        Err(ApplyError::Domain(error)) => Err(error),
        Err(ApplyError::Driver(error)) => Err(SandboxStoreError::Unavailable(error.to_string())),
    }
}
