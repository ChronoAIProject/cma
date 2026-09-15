//! Lifecycle effects have their own exact original-human proof and canonical
//! worker claim. Active renewal and retirement cleanup remain separate lanes.
mod audit;
mod authority;
mod lineage;
mod publish;
mod save;
use super::{MongoBridgeAllocationStore, MutationError};
use crate::{
    bridge::provisioning::revival::ChannelRevivalAuthority,
    sandbox::{channel_lifecycle_credentials::model::ChannelLifecycleSnapshot, CredentialMaterial},
    store::{
        durability::majority_collection,
        sandbox_mongo::{commit_transaction, is_transient, start_transaction, CommitOutcome},
        MongoSandboxCredentialStore,
    },
};
use layer2_core::{
    bridge::allocation::{
        AllocationError, AllocationRecord, AllocationState, ChannelSandboxSidecar,
    },
    SandboxId,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use std::time::SystemTime;

pub(crate) enum LifecycleCredentialChange<'a> {
    Read,
    MintIntent,
    Mint {
        custody: &'a MongoSandboxCredentialStore,
        material: &'a CredentialMaterial,
    },
    Deliver,
    Revoke {
        custody: &'a MongoSandboxCredentialStore,
    },
    Publish,
}
impl MongoBridgeAllocationStore {
    pub(crate) fn require_channel_lifecycle_database(
        &self,
        database: &str,
    ) -> Result<(), AllocationError> {
        if self.records.namespace().db != database {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
    pub(crate) async fn channel_lifecycle_origin(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<AllocationRecord, AllocationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let rows: Collection<Document> = majority_collection(&database, "sandboxes");
        let row = rows
            .find_one(doc! {"_id":sandbox_id.as_str()})
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Corrupt)?;
        let creation: ChannelSandboxSidecar = mongodb::bson::deserialize_from_bson(
            row.get("channel_creation")
                .cloned()
                .ok_or(AllocationError::IdentityMismatch)?,
        )
        .map_err(|_| AllocationError::Corrupt)?;
        let allocation = self
            .channel_allocation(&creation.allocation_id)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        if allocation.placement.target().sandbox_id != *sandbox_id {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(allocation)
    }
    pub(crate) async fn current_channel_lifecycle(
        &self,
        permit: &ChannelRevivalAuthority,
    ) -> Result<ChannelLifecycleSnapshot, AllocationError> {
        self.change_channel_lifecycle(permit, LifecycleCredentialChange::Read)
            .await
    }
    /// Called inside accepted-command revival admission's existing transaction.
    pub(crate) async fn fence_channel_revival(
        &self,
        permit: &ChannelRevivalAuthority,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        authority::read_fenced(self, permit, session)
            .await
            .map(|_| ())
    }
    pub(crate) async fn change_channel_lifecycle(
        &self,
        permit: &ChannelRevivalAuthority,
        change: LifecycleCredentialChange<'_>,
    ) -> Result<ChannelLifecycleSnapshot, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            permit.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = async {
                let (snapshot, document) =
                    authority::read_fenced(self, permit, &mut session).await?;
                if matches!(change, LifecycleCredentialChange::Read) {
                    return Ok(snapshot);
                }
                let database = self.bridges.client.database(&self.records.namespace().db);
                save::apply(
                    &database,
                    snapshot,
                    &document,
                    permit,
                    &change,
                    &mut session,
                )
                .await
            }
            .await;
            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error) if is_transient(&error) => continue,
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if let Err(error) = permit.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(snapshot),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
