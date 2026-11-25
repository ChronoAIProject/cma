//! Hold one authorization snapshot without changing its validation or retry budget.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceTarget,
};

use super::super::sandbox_mongo::{
    is_transient, start_transaction, MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use super::{snapshot, ReadError};

impl MongoSandboxStore {
    pub(crate) fn with_access_snapshot_session(mut self, enabled: bool) -> Self {
        self.access_snapshot_session = enabled;
        self
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
    let snapshot_session = store.access_snapshot_session;
    tracing::debug!(parent: None, snapshot_session, "reading resource authorization snapshot");
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .snapshot(snapshot_session)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        if !snapshot_session {
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        }
        let result =
            snapshot::read_with_concern(store, target, &mut session, snapshot_session).await;
        // Reads have no commit receipt to reconcile. Preserve the read result
        // if transaction cleanup fails; the driver still owns session cleanup.
        if !snapshot_session && session.abort_guarded().await.is_err() {
            tracing::warn!(parent: None, "authorization read transaction cleanup failed");
        }
        match result {
            Ok(snapshot) => return Ok(snapshot),
            Err(ReadError::Domain(error)) => return Err(error),
            Err(ReadError::Driver(error)) if is_transient(&error) => continue,
            Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
        }
    }
    Err(ResourceAccessStoreError::Unavailable)
}
