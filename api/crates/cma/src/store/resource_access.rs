//! Additive resource-policy and share data. Legacy aggregate records keep their
//! owner fields, revisions, profile pins, conversation and runtime attachment.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError, ResourceTarget,
};
use mongodb::bson::Document;
use mongodb::options::IndexOptions;
use mongodb::IndexModel;

use super::sandbox_mongo::{
    is_transient, start_transaction, MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

mod admission;
mod artifacts;
pub(super) mod execution;
pub(super) use admission::insert_admission_policy;
mod inventory;
pub(super) use artifacts::ensure_artifact_indexes;
pub(super) use inventory::{
    read as read_agent_inventory, read_shared as read_shared_agent_inventory,
};
mod mutation;
pub(super) mod personal_inventory;
pub(super) use mutation::fence_lineage;
mod observation;
pub(super) mod records;
mod sandbox_inventory;
mod snapshot;
pub(super) mod snapshot_lineage;
mod snapshot_session;

pub(super) async fn read_execution_snapshot(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
    session: &mut mongodb::ClientSession,
) -> Result<ResourceAccessSnapshot, ReadError> {
    snapshot::read(store, target, session).await
}

pub(super) fn read_inventory_policy(
    document: &Document,
) -> Result<layer2_core::resource_access::ResourcePolicyRecord, ResourceAccessStoreError> {
    records::read_policy(document)
}

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
    async fn personal_agent_inventory_fence(
        &self,
        agents: &[layer2_core::sandbox::Agent],
        parents: &[ResourceTarget],
    ) -> Result<
        Option<layer2_core::resource_access::PersonalAgentInventoryFence>,
        ResourceAccessStoreError,
    > {
        if !self.resource_list_batch_reads {
            return Ok(None);
        }
        personal_inventory::parents::fence(self, agents, parents)
            .await
            .map(Some)
    }

    async fn personal_agent_page_fence(
        &self,
        agents: &[layer2_core::sandbox::Agent],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessStoreError> {
        if !self.resource_list_batch_reads {
            return Ok(None);
        }
        personal_inventory::fence(self, agents).await.map(Some)
    }

    async fn organization_agent_page_fence(
        &self,
        agents: &[layer2_core::sandbox::Agent],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessStoreError> {
        if !self.resource_list_batch_reads {
            return Ok(None);
        }
        personal_inventory::fence(self, agents).await.map(Some)
    }

    async fn agent_observation_snapshots(
        &self,
        agents: &[layer2_core::AgentId],
    ) -> Result<
        Vec<Result<ResourceAccessSnapshot, ResourceAccessStoreError>>,
        ResourceAccessStoreError,
    > {
        observation::read(self, agents).await
    }

    async fn organization_sandboxes(
        &self,
        organization: &layer2_core::SubjectId,
        query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let result = sandbox_inventory::read(self, organization, query, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(page) => return Ok(page),
                Err(ReadError::Domain(error)) => return Err(error),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
            }
        }
        Err(ResourceAccessStoreError::Unavailable)
    }

    async fn shared_sandboxes(
        &self,
        recipient: &layer2_core::SubjectId,
        organizations: &[layer2_core::SubjectId],
        query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let result =
                sandbox_inventory::read_shared(self, recipient, organizations, query, &mut session)
                    .await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(page) => return Ok(page),
                Err(ReadError::Domain(error)) => return Err(error),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
            }
        }
        Err(ResourceAccessStoreError::Unavailable)
    }

    async fn organization_agents(
        &self,
        organization: &layer2_core::SubjectId,
        query: &layer2_core::sandbox::AgentListQuery,
        page: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        if self.resource_list_batch_reads {
            for _ in 0..TRANSACTION_ATTEMPTS {
                let mut session = self
                    .client
                    .start_session()
                    .snapshot(true)
                    .await
                    .map_err(|_| ResourceAccessStoreError::Unavailable)?;
                let result =
                    inventory::read_batch(self, organization, query, page, &mut session).await;
                match result {
                    Ok(page) => return Ok(page),
                    Err(ReadError::Domain(error)) => return Err(error),
                    Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                    Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
                }
            }
            return Err(ResourceAccessStoreError::Unavailable);
        }
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let result = inventory::read(self, organization, query, page, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(page) => return Ok(page),
                Err(ReadError::Domain(error)) => return Err(error),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
            }
        }
        Err(ResourceAccessStoreError::Unavailable)
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        snapshot_session::read(self, target).await
    }

    async fn resource_access_snapshots(
        &self,
        targets: &[ResourceTarget],
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        if self.resource_list_batch_reads
            && targets.len() <= usize::from(layer2_core::sandbox::store::MAX_PAGE_SIZE) + 1
            && targets
                .iter()
                .all(|target| matches!(target, ResourceTarget::Agent { .. }))
        {
            for _ in 0..TRANSACTION_ATTEMPTS {
                match personal_inventory::fence_targets(self, targets).await {
                    Ok(snapshots) => return Ok(snapshots),
                    Err(ReadError::Domain(error)) => return Err(error),
                    Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                    Err(ReadError::Driver(_)) => return Err(ResourceAccessStoreError::Unavailable),
                }
            }
            return Err(ResourceAccessStoreError::Unavailable);
        }
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let result: Result<Vec<_>, ReadError> = async {
                let mut snapshots = Vec::with_capacity(targets.len());
                for target in targets {
                    snapshots.push(snapshot::read(self, target, &mut session).await?);
                }
                Ok(snapshots)
            }
            .await;
            // These are read-only snapshots; abort releases the transaction
            // while preserving the same session for every row in the batch.
            let _ = session.abort_guarded().await;
            match result {
                Ok(snapshots) => return Ok(snapshots),
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
            "resource_share_link_recipient",
            mongodb::bson::doc! {"accepted_subjects":1,"revoked":1,"selector_kind":1,"selector_id":1},
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
