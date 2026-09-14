use std::future::Future;

use layer2_core::trigger_domain::LaunchStoreError;
use mongodb::bson::Timestamp;
use mongodb::ClientSession;

use crate::store::sandbox_mongo::wait_for_reconciliation;

use super::{unavailable, MongoTriggerDefinitionStore};

pub(super) async fn snapshot_session(
    store: &MongoTriggerDefinitionStore,
    at: Timestamp,
) -> Result<ClientSession, LaunchStoreError> {
    store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(at)
        .await
        .map_err(unavailable)
}

pub(super) async fn reconcile<T, F>(
    context: &'static str,
    commit_error: mongodb::error::Error,
    future: F,
) -> Result<T, LaunchStoreError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, LaunchStoreError>> + Send + 'static,
{
    let ambiguous = commit_error.to_string();
    wait_for_reconciliation(context, future)
        .await
        .map_err(|deadline| {
            LaunchStoreError::Unavailable(format!("{deadline}; ambiguous commit: {ambiguous}"))
        })?
}

pub(super) fn no_operation_time(context: &str, error: mongodb::error::Error) -> LaunchStoreError {
    LaunchStoreError::Unavailable(format!(
        "{context} remained unknown without an operation time: {error}"
    ))
}

pub(super) fn still_unknown(context: &str, error: &str) -> LaunchStoreError {
    LaunchStoreError::Unavailable(format!("{context} remained unknown: {error}"))
}
