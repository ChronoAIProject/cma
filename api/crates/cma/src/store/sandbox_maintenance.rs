//! Evidence-backed Mongo admission of automatic Sandbox maintenance.

use futures_util::TryStreamExt;
use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{
    validate_image_rotation_request, validate_inactivity_pause_request, Page, PageRequest,
    SandboxImageRotationRequest, SandboxInactivityPauseRequest, SandboxMaintenanceOutcome,
    SandboxMaintenanceStore, SandboxStoreError, MAX_PAGE_SIZE,
};
use mongodb::bson::doc;

use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;
use transaction::{apply, reconcile, ApplyError, MaintenanceRequest};

mod consented;
mod evidence;
#[cfg(test)]
pub(super) mod mongo_integration_tests;
mod retained_availability;
mod transaction;

#[async_trait::async_trait]
impl SandboxMaintenanceStore for MongoSandboxStore {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "status": "active" };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandboxes
            .find(filter)
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
        schedule(self, MaintenanceRequest::Inactivity(request)).await
    }

    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        validate_image_rotation_request(&request)?;
        schedule(self, MaintenanceRequest::Rotation(request)).await
    }
}

async fn schedule(
    store: &MongoSandboxStore,
    request: MaintenanceRequest,
) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
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
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        let receipt = match apply(store, &request, &mut session).await {
            Ok(receipt) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        if receipt.event.is_none() {
            let _ = session.abort_transaction().await;
            return Ok(receipt.outcome);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.outcome),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
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
                        Ok(receipt.outcome)
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
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "sandbox maintenance exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}
