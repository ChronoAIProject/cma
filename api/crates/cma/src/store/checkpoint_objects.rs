//! Durable, sandbox-confined authority for whole-home checkpoint archives.

use std::sync::Arc;

use layer2_core::StoreError;
use layer3_adapter::hosted::{
    CheckpointArchiveDownload, CheckpointArchiveReceipt, CheckpointArchiveStore,
    CheckpointArchiveStoreFactory, CheckpointArchiveUpload,
};
use layer3_adapter::{AdapterError, Result as AdapterResult};
use mongodb::bson::Document;
use mongodb::{Client, Collection};

use super::checkpoint_storage::CheckpointBlobStore;
use super::durability::majority_collection;
use super::schema::initialize_collections;

mod download;
mod model;
mod publication;
mod purge;
mod retention;
mod tombstones;
mod upload;

use model::{ensure_indexes, validate_sandbox_id, GATES, MANIFESTS, TOMBSTONES};
pub(super) use retention::{prune_evicted, RetentionError};

/// Mongo-backed archive authority shared by all sandbox-confined views.
#[derive(Clone)]
pub struct MongoCheckpointObjectStore {
    manifests: Collection<Document>,
    gates: Collection<Document>,
    #[allow(dead_code)]
    tombstones: Collection<Document>,
    #[allow(dead_code)]
    sandboxes: Collection<Document>,
    objects: Arc<dyn CheckpointBlobStore>,
    client: Client,
}

impl MongoCheckpointObjectStore {
    #[cfg(test)]
    pub(crate) async fn open(client: &Client, database_name: &str) -> Result<Self, StoreError> {
        Self::open_with_storage(
            client,
            database_name,
            super::checkpoint_storage::test_storage(database_name),
        )
        .await
    }

    pub(crate) async fn open_with_storage(
        client: &Client,
        database_name: &str,
        objects: Arc<dyn CheckpointBlobStore>,
    ) -> Result<Self, StoreError> {
        let database = client.database(database_name);
        initialize_collections(&database, &[MANIFESTS, GATES, TOMBSTONES]).await?;

        let manifests = majority_collection(&database, MANIFESTS);
        ensure_indexes(&manifests).await?;
        let tombstones = majority_collection(&database, TOMBSTONES);
        tombstones::ensure_indexes(&tombstones).await?;

        Ok(Self::handles(client, database_name, objects))
    }

    pub(in crate::store) fn handles(
        client: &Client,
        database_name: &str,
        objects: Arc<dyn CheckpointBlobStore>,
    ) -> Self {
        let database = client.database(database_name);
        Self {
            manifests: majority_collection(&database, MANIFESTS),
            gates: majority_collection(&database, GATES),
            tombstones: majority_collection(&database, TOMBSTONES),
            sandboxes: majority_collection(&database, super::sandbox_mongo::SANDBOXES),
            objects,
            client: client.clone(),
        }
    }

    /// Return an archive authority whose every operation is bound to one sandbox.
    pub fn for_sandbox(&self, sandbox_id: &str) -> AdapterResult<Arc<dyn CheckpointArchiveStore>> {
        validate_sandbox_id(sandbox_id)?;
        Ok(Arc::new(SandboxCheckpointArchiveStore {
            store: self.clone(),
            sandbox_id: sandbox_id.to_owned(),
        }))
    }

    /// Retire one sandbox and delete its external objects and durable upload stages.
    ///
    /// Work is deliberately bounded. `Unavailable` after partial progress asks the
    /// lifecycle worker to retry; each retry continues idempotently.
    #[allow(dead_code)]
    pub async fn purge_sandbox(&self, sandbox_id: &str) -> AdapterResult<()> {
        validate_sandbox_id(sandbox_id)?;
        purge::purge_sandbox(self, sandbox_id).await
    }

    #[allow(dead_code)]
    pub(crate) async fn cleanup_due(&self, now: std::time::SystemTime) -> AdapterResult<usize> {
        let now = mongodb::bson::DateTime::from_system_time(now);
        let retired = purge::retire_deleted_sandboxes(self, now).await;
        let manifests = purge::cleanup_due(self, now).await;
        let tombstones = tombstones::cleanup_due(self, now).await;
        Ok(retired? + manifests? + tombstones?)
    }
}

impl CheckpointArchiveStoreFactory for MongoCheckpointObjectStore {
    fn for_sandbox(&self, sandbox_id: &str) -> AdapterResult<Arc<dyn CheckpointArchiveStore>> {
        MongoCheckpointObjectStore::for_sandbox(self, sandbox_id)
    }
}

#[derive(Clone)]
struct SandboxCheckpointArchiveStore {
    store: MongoCheckpointObjectStore,
    sandbox_id: String,
}

#[async_trait::async_trait]
impl CheckpointArchiveStore for SandboxCheckpointArchiveStore {
    async fn upload(
        &self,
        archive: CheckpointArchiveUpload,
    ) -> AdapterResult<CheckpointArchiveReceipt> {
        upload::upload(&self.store, &self.sandbox_id, archive).await
    }

    async fn download(&self, object_reference: &str) -> AdapterResult<CheckpointArchiveDownload> {
        download::download(&self.store, &self.sandbox_id, object_reference).await
    }
}

fn unavailable(message: &'static str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

fn rejected(message: &'static str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

#[cfg(test)]
#[path = "checkpoint_objects/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "checkpoint_objects/mongo_integration_tests.rs"]
mod mongo_integration_tests;
