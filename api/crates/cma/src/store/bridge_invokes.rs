//! Explicit retained-Agent invoke grants. Grant mutation and effect fences use
//! the same Mongo transaction as current bridge, custody and resource identity.
use std::sync::Arc;

use futures_util::TryStreamExt;
use layer2_core::bridge::invoke::*;
use layer2_core::bridge::{BridgeError, BridgeId, BridgeRecord};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection, IndexModel};

use super::durability::majority_collection;
use super::resource_access::{fence_lineage, read_execution_snapshot, ReadError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
    start_transaction, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::{bridge_definitions::decode_bridge, MongoBridgeStore};
mod enable;
pub(super) use enable::fence_enable;

pub(crate) struct MongoBridgeInvokeStore {
    bridges: Arc<MongoBridgeStore>,
    resources: Arc<MongoSandboxStore>,
    grants: Collection<Document>,
    receipts: Collection<Document>,
    audits: Collection<Document>,
}

impl MongoBridgeInvokeStore {
    pub(crate) async fn open(
        bridges: Arc<MongoBridgeStore>,
        resources: Arc<MongoSandboxStore>,
        name: &str,
    ) -> Result<Self, layer2_core::StoreError> {
        if bridges.definitions.namespace().db != name || resources.sandboxes.namespace().db != name
        {
            return Err(layer2_core::StoreError::Unavailable(
                "invoke storage namespace mismatch".into(),
            ));
        }
        bridges.invoke_resources.get_or_init(|| resources.clone());
        let database = bridges.client.database(name);
        super::schema::initialize_collections(
            &database,
            &[
                "bridge_invoke_grants",
                "bridge_invoke_receipts",
                "bridge_invoke_audits",
            ],
        )
        .await?;
        let store = Self {
            bridges,
            resources,
            grants: majority_collection(&database, "bridge_invoke_grants"),
            receipts: majority_collection(&database, "bridge_invoke_receipts"),
            audits: majority_collection(&database, "bridge_invoke_audits"),
        };
        store
            .grants
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"bridge_id":1_i32,"_id":1_i32})
                    .build(),
            )
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create invoke grant index".into())
            })?;
        Ok(store)
    }

    /// Called inside the allocator/command store's transaction immediately before
    /// its effect admission. A proof never authorizes a different operation; the
    /// consumer also checks its exact effect reference against its write.
    pub(crate) async fn fence(
        &self,
        proof: &BridgeInvokeProof,
        session: &mut ClientSession,
    ) -> Result<(), BridgeInvokeError> {
        self.fence_transaction(proof, session)
            .await
            .map_err(MutationError::into_invoke)
    }

    async fn fence_transaction(
        &self,
        proof: &BridgeInvokeProof,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        proof.require_current()?;
        let bridge = self.read_bridge(&proof.bridge().id, session).await?;
        let grant = self
            .grants
            .find_one(doc! {"_id":proof.grant().id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_grant)
            .transpose()?
            .ok_or(BridgeInvokeError::NotFound)?;
        grant.require_active()?;
        let resources =
            read_execution_snapshot(&self.resources, &grant.target.resource(), session).await?;
        proof.require_snapshot(&bridge, &grant, &resources)?;
        self.bridges
            .credentials
            .require_retained_binding(&bridge, &grant.config, proof.custody_pin(), session)
            .await?;
        self.fence_bridge(&bridge, session).await?;
        let changed = self
            .grants
            .update_one(
                doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64},
                doc! {"$inc":{"effect_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        fence_lineage(&self.resources, &resources, session).await?;
        proof.require_current()?;
        Ok(())
    }

    async fn read_bridge(
        &self,
        id: &BridgeId,
        session: &mut ClientSession,
    ) -> Result<BridgeRecord, MutationError> {
        let document = self
            .bridges
            .definitions
            .find_one(doc! {"_id":id.as_str()})
            .session(session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        Ok(decode_bridge(&document)?)
    }

    async fn fence_bridge(
        &self,
        bridge: &BridgeRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let result = self
            .bridges
            .definitions
            .update_one(
                doc! {"_id":bridge.id.as_str(),"revision":bridge.revision as i64},
                doc! {"$inc":{"invoke_fence":1_i64}},
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        Ok(())
    }

    async fn transaction(
        &self,
        write: &BridgeInvokeWrite,
        session: &mut ClientSession,
    ) -> Result<BridgeInvokeReceipt, MutationError> {
        write.require_current()?;
        let bridge = self
            .read_bridge(&write.management().record().id, session)
            .await?;
        if bridge != *write.management().record() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        if let Some(document) = self
            .receipts
            .find_one(doc! {"_id":&write.receipt().id})
            .session(&mut *session)
            .await?
        {
            return Ok(match_invoke_replay(
                &read_receipt(&document)?,
                write.receipt(),
            )?);
        }
        let current = self
            .grants
            .find_one(doc! {"_id":write.record().id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_grant)
            .transpose()?;
        if current.as_ref() != write.previous() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        if let Some(observed) = write.resources() {
            let resources = read_execution_snapshot(
                &self.resources,
                &write.record().target.resource(),
                session,
            )
            .await?;
            if &resources != observed {
                return Err(BridgeInvokeError::TargetMismatch.into());
            }
            write
                .record()
                .target
                .require(&write.record().owner, &resources)?;
            self.bridges
                .credentials
                .require_binding(
                    &bridge,
                    write.custody_pin().ok_or(BridgeInvokeError::Corrupt)?,
                    session,
                )
                .await?;
            fence_lineage(&self.resources, &resources, session).await?;
        }
        self.fence_bridge(&bridge, session).await?;
        let document = grant_document(write.record())?;
        if let Some(previous) = write.previous() {
            let result = self
                .grants
                .replace_one(
                    doc! {"_id":previous.id.as_str(),"revision":previous.revision as i64},
                    document,
                )
                .session(&mut *session)
                .await?;
            if result.matched_count != 1 {
                return Err(BridgeInvokeError::Conflict.into());
            }
        } else {
            self.grants
                .insert_one(document)
                .session(&mut *session)
                .await?;
        }
        self.audits
            .insert_one(
                doc! {"_id":format!("{}:{}",write.record().id.as_str(),write.record().revision),
                "document_schema_version":DOCUMENT_SCHEMA_VERSION,"receipt_id":&write.receipt().id,
                "request_digest":&write.receipt().request_digest,
                "record":record_bson(write.record()).map_err(|_|BridgeInvokeError::Corrupt)?},
            )
            .session(&mut *session)
            .await?;
        self.receipts
            .insert_one(
                doc! {"_id":&write.receipt().id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "record":record_bson(write.receipt()).map_err(|_|BridgeInvokeError::Corrupt)?},
            )
            .session(&mut *session)
            .await?;
        write.require_current()?;
        Ok(write.receipt().clone())
    }
}

#[async_trait::async_trait]
impl BridgeInvokeStore for MongoBridgeInvokeStore {
    async fn grant(
        &self,
        id: &BridgeInvokeGrantId,
    ) -> Result<Option<BridgeInvokeGrant>, BridgeInvokeError> {
        id.validate()?;
        self.grants
            .find_one(doc! {"_id":id.as_str()})
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?
            .as_ref()
            .map(read_grant)
            .transpose()
    }
    async fn receipt(&self, id: &str) -> Result<Option<BridgeInvokeReceipt>, BridgeInvokeError> {
        self.receipts
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?
            .as_ref()
            .map(read_receipt)
            .transpose()
    }
    async fn list(
        &self,
        bridge: &BridgeId,
        after: Option<&BridgeInvokeGrantId>,
        limit: u32,
    ) -> Result<Vec<BridgeInvokeGrant>, BridgeInvokeError> {
        if !(1..=100).contains(&limit) {
            return Err(BridgeInvokeError::Invalid("limit"));
        }
        let mut query = doc! {"bridge_id":bridge.as_str()};
        if let Some(after) = after {
            after.validate()?;
            query.insert("_id", doc! {"$gt":after.as_str()});
        }
        let documents = self
            .grants
            .find(query)
            .sort(doc! {"_id":1_i32})
            .limit(i64::from(limit) + 1)
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?
            .try_collect::<Vec<Document>>()
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        documents.iter().map(read_grant).collect()
    }
    async fn apply(
        &self,
        write: BridgeInvokeWrite,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
        for _ in 0..8 {
            write.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = match self.transaction(&write, &mut session).await {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        other => return Err(other.into_invoke()),
                    }
                }
            };
            write.require_current()?;
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return self
                        .receipt(&write.receipt().id)
                        .await?
                        .map(|stored| match_invoke_replay(&stored, write.receipt()))
                        .transpose()?
                        .ok_or(BridgeInvokeError::Unavailable);
                }
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }
}

fn grant_document(grant: &BridgeInvokeGrant) -> Result<Document, BridgeInvokeError> {
    grant.validate()?;
    Ok(
        doc! {"_id":grant.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "bridge_id":grant.pin.bridge_id.as_str(),"revision":grant.revision as i64,
        "record":record_bson(grant).map_err(|_|BridgeInvokeError::Corrupt)?},
    )
}
fn read_grant(document: &Document) -> Result<BridgeInvokeGrant, BridgeInvokeError> {
    let grant: BridgeInvokeGrant = read_record(document).map_err(|_| BridgeInvokeError::Corrupt)?;
    let expected = grant_document(&grant)?;
    if ["_id", "bridge_id", "revision"]
        .iter()
        .any(|key| document.get(*key) != expected.get(*key))
    {
        return Err(BridgeInvokeError::Corrupt);
    }
    Ok(grant)
}
fn read_receipt(document: &Document) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
    let receipt: BridgeInvokeReceipt =
        read_record(document).map_err(|_| BridgeInvokeError::Corrupt)?;
    receipt.validate()?;
    if document.get_str("_id").ok() != Some(receipt.id.as_str()) {
        return Err(BridgeInvokeError::Corrupt);
    }
    Ok(receipt)
}

enum MutationError {
    Invoke(BridgeInvokeError),
    Mongo(mongodb::error::Error),
}
impl MutationError {
    fn into_invoke(self) -> BridgeInvokeError {
        match self {
            Self::Invoke(error) => error,
            Self::Mongo(_) => BridgeInvokeError::Unavailable,
        }
    }
}
impl From<BridgeInvokeError> for MutationError {
    fn from(value: BridgeInvokeError) -> Self {
        Self::Invoke(value)
    }
}
impl From<BridgeError> for MutationError {
    fn from(value: BridgeError) -> Self {
        Self::Invoke(value.into())
    }
}
impl From<mongodb::error::Error> for MutationError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Mongo(value)
    }
}
impl From<ReadError> for MutationError {
    fn from(value: ReadError) -> Self {
        match value {
            ReadError::Driver(error) => Self::Mongo(error),
            ReadError::Domain(
                layer2_core::resource_access::ResourceAccessStoreError::Unavailable,
            ) => Self::Invoke(BridgeInvokeError::Unavailable),
            ReadError::Domain(_) => Self::Invoke(BridgeInvokeError::TargetMismatch),
        }
    }
}

#[cfg(test)]
mod tests;
