//! Additive resource-policy and share data. Legacy aggregate records keep their
//! owner fields, revisions, profile pins, conversation and runtime attachment.

use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError, ResourceTarget,
};
use mongodb::bson::Document;
use mongodb::options::IndexOptions;
use mongodb::IndexModel;

use super::sandbox_mongo::{
    is_transient, start_transaction, MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

mod mutation;
mod records;
mod snapshot;

pub(super) enum ReadError {
    Domain(ResourceAccessStoreError),
    Driver(mongodb::error::Error),
}
impl From<ResourceAccessStoreError> for ReadError {
    fn from(value: ResourceAccessStoreError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for ReadError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}

#[async_trait::async_trait]
impl ResourceAccessStore for MongoSandboxStore {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let result = snapshot::read(self, target, &mut session).await;
            // This transaction performed reads only. Abort releases the snapshot;
            // no write receipt or commit reconciliation is needed.
            let _ = session.abort_transaction().await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(ReadError::Domain(error)) => return Err(error),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
            }
        }
        Err(ResourceAccessStoreError::Unavailable)
    }
}

pub(super) async fn ensure_indexes(
    store: &MongoSandboxStore,
) -> Result<(), layer2_core::StoreError> {
    for (collection, name, keys) in [
        (
            &store.resource_policies,
            "resource_policy_owner",
            mongodb::bson::doc! {"owner_kind":1,"owner_subject":1,"_id":1},
        ),
        (
            &store.resource_shares,
            "resource_share_selector",
            mongodb::bson::doc! {"sandbox_id":1,"revoked":1,"selector_kind":1,"selector_id":1},
        ),
        (
            &store.resource_shares,
            "resource_share_recipient",
            mongodb::bson::doc! {"target_org_id":1,"revoked":1,"_id":1},
        ),
    ] {
        ensure_index(collection, name, keys).await?;
    }
    Ok(())
}
async fn ensure_index(
    collection: &mongodb::Collection<Document>,
    name: &str,
    keys: Document,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(keys)
                .options(IndexOptions::builder().name(name.to_owned()).build())
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create resource authorization indexes".into())
        })
}
