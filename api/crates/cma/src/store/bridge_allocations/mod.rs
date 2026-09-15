//! Transactional partition heads and immutable allocation generations. No
//! provider creation or Agent command submission occurs in this store.
mod channel_children;
pub(crate) mod channel_lifecycle;
mod channel_provisioning;
mod channel_renewal;
mod channel_startup;
mod records;
mod sandbox_admission;
mod transaction;

use std::sync::Arc;

use layer2_core::bridge::allocation::{
    AllocationAuthority, AllocationError, AllocationIdentity, AllocationMutation, AllocationRecord,
    AllocationReservation, AllocationStore,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, IndexModel,
};

use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    },
    schema::initialize_collections,
    MongoBridgeInvokeStore, MongoBridgeStore,
};

use records::decode_head;
pub(super) use records::decode_record;

#[derive(Clone)]
pub(crate) struct MongoBridgeAllocationStore {
    bridges: Arc<MongoBridgeStore>,
    invokes: Option<Arc<MongoBridgeInvokeStore>>,
    heads: Collection<Document>,
    records: Collection<Document>,
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn open(
        bridges: Arc<MongoBridgeStore>,
        invokes: Option<Arc<MongoBridgeInvokeStore>>,
        name: &str,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = bridges.client.database(name);
        initialize_collections(
            &database,
            &[
                "bridge_allocation_heads",
                "bridge_allocations",
                "channel_credential_renewal_audits",
                "channel_lifecycle_credential_audits",
            ],
        )
        .await?;
        let store = Self {
            bridges,
            invokes,
            heads: majority_collection(&database, "bridge_allocation_heads"),
            records: majority_collection(&database, "bridge_allocations"),
        };
        store.records.create_index(IndexModel::builder()
            .keys(doc! {"bridge_id":1_i32,"key_hash":1_i32,"key_version":1_i32,"generation":1_i32})
            .options(mongodb::options::IndexOptions::builder().unique(true).build()).build())
            .await.map_err(|_| layer2_core::StoreError::Unavailable("create allocation identity index".into()))?;
        store
            .records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"bridge_id":1_i32,"creates_sandbox":1_i32})
                    .build(),
            )
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create allocation capacity index".into())
            })?;
        store
            .heads
            .create_index(IndexModel::builder().keys(doc! {"bridge_id":1_i32}).build())
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create partition capacity index".into())
            })?;
        Ok(store)
    }

    async fn fence(
        &self,
        authority: &AllocationAuthority,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        authority.require_current()?;
        if let AllocationAuthority::Existing(proof) = authority {
            self.invokes
                .as_ref()
                .ok_or(AllocationError::AuthorityUnavailable)?
                .fence(proof, &mut *session)
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
        }
        let current = self
            .bridges
            .definitions
            .find_one(doc! {"_id":authority.bridge().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::AuthorityUnavailable)?;
        let current = super::bridge_definitions::decode_bridge(&current)
            .map_err(|_| AllocationError::Corrupt)?;
        authority.require_snapshot(&current)?;
        self.bridges
            .credentials
            .require_retained_binding(
                &current,
                authority.config()?,
                authority.custody_pin(),
                &mut *session,
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let changed = self
            .bridges
            .definitions
            .update_one(
                doc! {"_id":current.id.as_str(),"revision":current.revision as i64},
                doc! {"$inc":{"allocation_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        authority.require_current()?;
        Ok(())
    }

    async fn perform(&self, write: Write<'_>) -> Result<AllocationRecord, AllocationError> {
        for _ in 0..8 {
            write.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = match self.transaction(&write, &mut session).await {
                Ok(record) => record,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                // The deterministic identity makes a later retry reconcile the
                // same record. An ambiguous commit never permits a new tree.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}

#[async_trait::async_trait]
impl AllocationStore for MongoBridgeAllocationStore {
    async fn current(
        &self,
        identity: &AllocationIdentity,
    ) -> Result<Option<AllocationRecord>, AllocationError> {
        let Some(head) = self
            .heads
            .find_one(doc! {"_id":identity.head_id()?})
            .await
            .map_err(|_| AllocationError::Unavailable)?
        else {
            return Ok(None);
        };
        let id = decode_head(&head, identity)?;
        let document = self
            .records
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Corrupt)?;
        let record = decode_record(&document)?;
        if record.identity.key != identity.key || record.identity.bridge_id != identity.bridge_id {
            return Err(AllocationError::Corrupt);
        }
        Ok(Some(record))
    }
    async fn generation(
        &self,
        identity: &AllocationIdentity,
    ) -> Result<Option<AllocationRecord>, AllocationError> {
        self.records
            .find_one(doc! {"_id":identity.id()?})
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .map(|document| decode_record(&document))
            .transpose()
    }
    async fn reserve(
        &self,
        write: AllocationReservation,
    ) -> Result<AllocationRecord, AllocationError> {
        self.perform(Write::Reserve(&write)).await
    }
    async fn mutate(&self, write: AllocationMutation) -> Result<AllocationRecord, AllocationError> {
        self.perform(Write::Mutate(&write)).await
    }
}

enum Write<'a> {
    Reserve(&'a AllocationReservation),
    Mutate(&'a AllocationMutation),
}
impl Write<'_> {
    fn require_current(&self) -> Result<(), AllocationError> {
        match self {
            Self::Reserve(write) => write.require_current(),
            Self::Mutate(write) => write.require_current(),
        }
    }
    fn authority(&self) -> &AllocationAuthority {
        match self {
            Self::Reserve(write) => write.authority(),
            Self::Mutate(write) => write.authority(),
        }
    }
    fn require_time(&self) -> Result<(), AllocationError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| AllocationError::Unavailable)?
            .as_millis();
        let now = i64::try_from(now).map_err(|_| AllocationError::Unavailable)?;
        match self {
            Self::Reserve(write) => write.require_at(now),
            Self::Mutate(write) => write.require_at(now),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MutationError {
    #[error(transparent)]
    Core(#[from] AllocationError),
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
}
