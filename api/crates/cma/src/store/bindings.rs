//! Durable binding reservations share the Sandbox transaction boundary.
use futures_util::TryStreamExt;
use layer2_core::sandbox::binding::store::{
    binding_has_work, conversation_has_work, conversation_storage_key, validate_binding_write,
    validate_conversation_write, validate_page_limit,
};
use layer2_core::sandbox::binding::{BindingConversation, BindingStore, ProviderBinding};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Collection, Database, IndexModel,
};

use super::durability::majority_collection;
use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore};
use super::schema::{initialize_collections, DOCUMENT_SCHEMA_VERSION};
mod admission;
mod conversations;
mod custody;
mod rotation;
pub(crate) use custody::MongoBindingCredentials;
mod documents;
use documents::*;
#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod mongo_tests;

const BINDINGS: &str = "provider_bindings";
const CONVERSATIONS: &str = "binding_conversations";

#[derive(Clone)]
pub(super) struct BindingCollections {
    pub(super) records: Collection<Document>,
    pub(super) conversations: Collection<Document>,
}

impl BindingCollections {
    pub(super) async fn open(database: &Database) -> Result<Self, StoreError> {
        initialize_collections(database, &[BINDINGS, CONVERSATIONS]).await?;
        let value = Self {
            records: majority_collection(database, BINDINGS),
            conversations: majority_collection(database, CONVERSATIONS),
        };
        for (collection, keys) in [
            (&value.records, doc! {"pending": 1, "_id": 1}),
            (&value.records, doc! {"record.owner": 1, "_id": 1}),
            (&value.conversations, doc! {"pending": 1, "_id": 1}),
            (
                &value.conversations,
                doc! {"binding_id": 1, "generation": 1, "conversation_id": 1},
            ),
        ] {
            collection
                .create_index(IndexModel::builder().keys(keys).build())
                .await
                .map_err(|e| StoreError::Unavailable(format!("binding index: {e}")))?;
        }
        for (collection, key) in [
            (&value.records, "record.key_sha256"),
            (&value.conversations, "record.agent_id"),
        ] {
            let mut keys = Document::new();
            keys.insert(key, 1);
            collection
                .create_index(
                    IndexModel::builder()
                        .keys(keys)
                        .options(
                            mongodb::options::IndexOptions::builder()
                                .unique(true)
                                .build(),
                        )
                        .build(),
                )
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("binding unique index: {error}"))
                })?;
        }
        Ok(value)
    }
}

#[async_trait::async_trait]
impl BindingStore for MongoSandboxStore {
    async fn find_by_key_hash(
        &self,
        hash: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        self.bindings
            .records
            .find_one(doc! {"record.key_sha256": hash, "record.state.status": {"$ne": "retired"}})
            .await
            .map_err(driver)?
            .map(read_binding)
            .transpose()
    }
    async fn binding_conversation_for_agent(
        &self,
        binding: &str,
        generation: u64,
        agent: &layer2_core::AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        self.bindings.conversations.find_one(doc! {"binding_id": binding, "generation": integer(generation)?, "record.agent_id": agent.as_str()})
            .await.map_err(driver)?.map(read_conversation).transpose()
    }

    async fn binding(&self, id: &str) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        self.bindings
            .records
            .find_one(doc! {"_id": id})
            .await
            .map_err(driver)?
            .map(read_binding)
            .transpose()
    }

    async fn save_binding(
        &self,
        expected: Option<u64>,
        next: ProviderBinding,
    ) -> Result<(), SandboxStoreError> {
        let current = self.binding(&next.binding_id).await?;
        validate_binding_write(current.as_ref(), expected, &next)?;
        let mut document = binding_document(&next)?;
        if let Some(expected) = expected {
            document.remove("_id");
            let result = self.bindings.records.update_one(
                doc! {"_id": &next.binding_id, "revision": integer(expected)?, "document_schema_version": DOCUMENT_SCHEMA_VERSION},
                doc! {"$set": document}).await.map_err(driver)?;
            if result.matched_count != 1 {
                return Err(SandboxStoreError::Conflict);
            }
        } else {
            self.bindings
                .records
                .insert_one(document)
                .await
                .map_err(driver)?;
        }
        Ok(())
    }

    async fn pending_bindings(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError> {
        validate_page_limit(limit)?;
        let mut query = doc! {"pending": true};
        if let Some(after) = after {
            query.insert("_id", doc! {"$gt": after});
        }
        let docs: Vec<Document> = self
            .bindings
            .records
            .find(query)
            .sort(doc! {"_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(driver)?
            .try_collect()
            .await
            .map_err(driver)?;
        docs.into_iter().map(read_binding).collect()
    }

    async fn binding_conversation(
        &self,
        binding: &str,
        generation: u64,
        id: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        self.bindings
            .conversations
            .find_one(doc! {"_id": format!("{binding}:{generation:020}:{id}")})
            .await
            .map_err(driver)?
            .map(read_conversation)
            .transpose()
    }

    async fn save_binding_conversation(
        &self,
        binding_revision: u64,
        expected: Option<u64>,
        next: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        self.commit_binding_conversation(binding_revision, expected, next)
            .await
    }

    async fn binding_conversations(
        &self,
        binding: &str,
        generation: u64,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        validate_page_limit(limit)?;
        let mut query = doc! {"binding_id":binding, "generation":integer(generation)?};
        if let Some(after) = after {
            query.insert("conversation_id", doc! {"$gt":after});
        }
        let docs: Vec<Document> = self
            .bindings
            .conversations
            .find(query)
            .sort(doc! {"conversation_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(driver)?
            .try_collect()
            .await
            .map_err(driver)?;
        docs.into_iter().map(read_conversation).collect()
    }

    async fn pending_binding_conversations(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        validate_page_limit(limit)?;
        let mut query = doc! {"pending":true};
        if let Some(after) = after {
            query.insert("_id", doc! {"$gt":after});
        }
        let docs: Vec<Document> = self
            .bindings
            .conversations
            .find(query)
            .sort(doc! {"_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(driver)?
            .try_collect()
            .await
            .map_err(driver)?;
        docs.into_iter().map(read_conversation).collect()
    }
}

fn driver(error: mongodb::error::Error) -> SandboxStoreError {
    if is_duplicate_key(&error) || super::sandbox_mongo::is_transient(&error) {
        SandboxStoreError::Conflict
    } else {
        SandboxStoreError::Unavailable(error.to_string())
    }
}

#[cfg(test)]
mod access_tests;
#[cfg(test)]
mod custody_tests;
#[cfg(test)]
mod rotation_tests;
