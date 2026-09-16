//! Durable bridge definitions and idempotent management receipts. Runtime
//! dispatch and secret custody are separate stores and authority boundaries.

pub(in crate::store) mod inbox;
pub(in crate::store) use inbox::decode as decode_inbox;
pub(crate) use inbox::MongoCallbackInbox;

use futures_util::TryStreamExt;
use layer2_core::bridge::{
    match_bridge_replay, BridgeError, BridgeId, BridgeRecord, BridgeStore, BridgeWrite,
    StoredBridgeReceipt,
};
use layer2_core::resource_access::ResourceOwner;
use mongodb::bson::{doc, Document};
use mongodb::{Client, ClientSession, Collection, IndexModel};
use std::sync::Arc;

use super::durability::majority_collection;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
    require_transaction_topology, start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
};
use super::schema::initialize_collections;

#[derive(Clone)]
pub struct MongoBridgeStore {
    pub(super) client: Client,
    pub(super) definitions: Collection<Document>,
    receipts: Collection<Document>,
    pub(super) credentials: Arc<super::MongoBridgeCredentialStore>,
    pub(super) invoke_resources: Arc<std::sync::OnceLock<Arc<super::MongoSandboxStore>>>,
}

impl MongoBridgeStore {
    pub(super) async fn open(
        client: &Client,
        name: &str,
        deployment: &str,
        key: &super::AuthEncryptionKey,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = client.database(name);
        require_transaction_topology(&database).await?;
        initialize_collections(
            &database,
            &["bridge_definitions", "bridge_definition_receipts"],
        )
        .await?;
        let store = Self {
            client: client.clone(),
            definitions: majority_collection(&database, "bridge_definitions"),
            receipts: majority_collection(&database, "bridge_definition_receipts"),
            credentials: Arc::new(
                super::MongoBridgeCredentialStore::open(client, name, deployment, key).await?,
            ),
            invoke_resources: Arc::new(std::sync::OnceLock::new()),
        };
        store
            .definitions
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "owner_scope": 1_i32, "issuer": 1_i32, "_id": 1_i32 })
                    .build(),
            )
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create bridge owner index".into())
            })?;
        Ok(store)
    }

    pub(super) fn credentials(&self) -> Arc<super::MongoBridgeCredentialStore> {
        self.credentials.clone()
    }

    async fn transaction(
        &self,
        write: &BridgeWrite,
        session: &mut ClientSession,
    ) -> Result<StoredBridgeReceipt, MutationError> {
        write.require_current()?;
        if let Some(document) = self
            .receipts
            .find_one(doc! { "_id": &write.receipt().id })
            .session(&mut *session)
            .await?
        {
            return Ok(match_bridge_replay(
                &decode_receipt(&document)?,
                write.receipt(),
            )?);
        }
        let current = self
            .definitions
            .find_one(doc! { "_id": write.record().id.as_str() })
            .session(&mut *session)
            .await?
            .as_ref()
            .map(decode_bridge)
            .transpose()?;
        if current.as_ref().map_or(0, |r| r.revision) != write.expected_revision()
            || current
                .as_ref()
                .is_some_and(|r| r.owner != write.record().owner)
        {
            return Err(BridgeError::Conflict.into());
        }
        if let Some(pin) = write.custody_pin() {
            self.credentials
                .require_binding(write.record(), pin, session)
                .await?;
        }
        if let Some(proof) = write.enable_grant() {
            super::bridge_invokes::fence_enable(
                self,
                proof,
                current.as_ref().ok_or(BridgeError::Conflict)?,
                session,
            )
            .await?;
        }
        if let Some(current) = &current {
            super::bridge_invokes::apply_management_usage(self, current, write, session)
                .await
                .map_err(|error| match error {
                    super::bridge_invokes::MutationError::Mongo(error) => {
                        MutationError::Mongo(error)
                    }
                    super::bridge_invokes::MutationError::Invoke(error) => {
                        MutationError::Core(match error {
                            layer2_core::bridge::invoke::BridgeInvokeError::Conflict => {
                                BridgeError::Conflict
                            }
                            layer2_core::bridge::invoke::BridgeInvokeError::Corrupt
                            | layer2_core::bridge::invoke::BridgeInvokeError::TargetMismatch => {
                                BridgeError::Corrupt
                            }
                            _ => BridgeError::Unavailable,
                        })
                    }
                })?;
        }
        let document = encode_bridge(write.record())?;
        if current.is_some() {
            let changed = self.definitions.replace_one(doc! { "_id": write.record().id.as_str(), "revision": write.expected_revision() as i64 }, document)
                .session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(BridgeError::Conflict.into());
            }
        } else {
            self.definitions
                .insert_one(document)
                .session(&mut *session)
                .await?;
        }
        self.receipts
            .insert_one(doc! {
                "_id": &write.receipt().id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "record": record_bson(write.receipt()).map_err(|_| BridgeError::Corrupt)?,
            })
            .session(&mut *session)
            .await?;
        write.require_current()?;
        Ok(write.receipt().clone())
    }
}

#[async_trait::async_trait]
impl BridgeStore for MongoBridgeStore {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        self.definitions
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|_| BridgeError::Unavailable)?
            .as_ref()
            .map(decode_bridge)
            .transpose()
    }

    async fn receipt(&self, id: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        self.receipts
            .find_one(doc! { "_id": id })
            .await
            .map_err(|_| BridgeError::Unavailable)?
            .as_ref()
            .map(decode_receipt)
            .transpose()
    }

    async fn list(
        &self,
        owner: &ResourceOwner,
        issuer: &str,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        if !(1..=100).contains(&limit) {
            return Err(BridgeError::InvalidConfiguration("limit"));
        }
        let mut filter = doc! { "owner_scope": record_bson(owner).map_err(|_| BridgeError::Corrupt)?, "issuer": issuer };
        if let Some(after) = after {
            filter.insert("_id", doc! { "$gt": after.as_str() });
        }
        let documents = self
            .definitions
            .find(filter)
            .sort(doc! { "_id": 1_i32 })
            .limit(i64::from(limit) + 1)
            .await
            .map_err(|_| BridgeError::Unavailable)?
            .try_collect::<Vec<Document>>()
            .await
            .map_err(|_| BridgeError::Unavailable)?;
        documents.iter().map(decode_bridge).collect()
    }

    async fn apply(&self, write: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        for _ in 0..8 {
            write.require_current()?;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| BridgeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeError::Unavailable)?;
            let receipt = match self.transaction(&write, &mut session).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(BridgeError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    // A durable receipt, if visible, reconciles a lost commit
                    // reply. Absence remains uncertain and is safe to retry.
                    return match self.receipt(&write.receipt().id).await? {
                        Some(stored) => match_bridge_replay(&stored, write.receipt()),
                        None => Err(BridgeError::Unavailable),
                    };
                }
            }
        }
        Err(BridgeError::Unavailable)
    }
}

fn encode_bridge(record: &BridgeRecord) -> Result<Document, BridgeError> {
    record.validate()?;
    Ok(doc! {
        "_id": record.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": record.revision as i64,
        "issuer": &record.current()?.config.connector.issuer,
        "owner_scope": record_bson(&record.owner).map_err(|_| BridgeError::Corrupt)?,
        "record": record_bson(record).map_err(|_| BridgeError::Corrupt)?,
    })
}

pub(super) fn decode_bridge(document: &Document) -> Result<BridgeRecord, BridgeError> {
    let record: BridgeRecord = read_record(document).map_err(|_| BridgeError::Corrupt)?;
    record.validate()?;
    let expected = encode_bridge(&record)?;
    if ["_id", "revision", "owner_scope", "issuer"]
        .iter()
        .any(|field| document.get(*field) != expected.get(*field))
    {
        return Err(BridgeError::Corrupt);
    }
    Ok(record)
}

fn decode_receipt(document: &Document) -> Result<StoredBridgeReceipt, BridgeError> {
    let receipt: StoredBridgeReceipt = read_record(document).map_err(|_| BridgeError::Corrupt)?;
    if document.get_str("_id").ok() != Some(receipt.id.as_str())
        || [&receipt.id, &receipt.command_digest]
            .iter()
            .any(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || receipt.receipt.revision == 0
        || receipt.receipt.config_revision == 0
        || receipt.receipt.config_revision > receipt.receipt.revision
    {
        return Err(BridgeError::Corrupt);
    }
    Ok(receipt)
}

#[derive(Debug, thiserror::Error)]
enum MutationError {
    #[error(transparent)]
    Core(#[from] BridgeError),
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
}

#[cfg(test)]
mod channel_control_compatibility_tests;
#[cfg(test)]
mod mongo_integration_tests;
