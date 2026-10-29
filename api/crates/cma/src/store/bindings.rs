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
mod associations;
mod commands;
mod conversations;
mod custody;
mod reprovision;
mod retry;
mod rotation;
mod tools;
pub(crate) use custody::MongoBindingCredentials;
pub(crate) use tools::MongoProviderToolSource;
mod documents;
use documents::*;
#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod association_tests;
#[cfg(test)]
mod existing_tests;
#[cfg(test)]
mod mongo_tests;

const BINDINGS: &str = "provider_bindings";
const CONVERSATIONS: &str = "binding_conversations";
const TOOL_OPENS: &str = "provider_tool_opens";

#[derive(Clone)]
pub(super) struct BindingCollections {
    pub(super) records: Collection<Document>,
    pub(super) conversations: Collection<Document>,
    pub(super) tool_opens: Collection<Document>,
    pub(super) preparations: Collection<Document>,
}

impl BindingCollections {
    pub(super) async fn open(database: &Database) -> Result<Self, StoreError> {
        initialize_collections(
            database,
            &[
                BINDINGS,
                CONVERSATIONS,
                TOOL_OPENS,
                reprovision::PREPARATIONS,
            ],
        )
        .await?;
        let value = Self {
            records: majority_collection(database, BINDINGS),
            conversations: majority_collection(database, CONVERSATIONS),
            tool_opens: majority_collection(database, TOOL_OPENS),
            preparations: majority_collection(database, reprovision::PREPARATIONS),
        };
        for (collection, keys) in [
            (&value.records, doc! {"pending": 1, "_id": 1}),
            (
                &value.records,
                doc! {"record.state.status": 1, "record.work": 1, "_id": 1},
            ),
            (&value.records, doc! {"key_history": 1}),
            (&value.records, doc! {"record.owner": 1, "_id": 1}),
            (&value.conversations, doc! {"pending": 1, "_id": 1}),
            (&value.conversations, doc! {"record.binding_id": 1}),
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
        value
            .records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"record.key_sha256": 1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| StoreError::Unavailable(format!("binding unique index: {error}")))?;
        value
            .preparations
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"binding_id":1,"generation":1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("binding preparation index: {error}"))
            })?;
        value
            .conversations
            .create_index(
                IndexModel::builder()
                    .keys(associations::index_keys())
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name(associations::INDEX.to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("binding association index: {error}"))
            })?;
        // An existing Agent may be borrowed by another channel. Lookup remains
        // unique inside one binding generation; resource lifecycle is retained
        // by the Agent's original owner, independently of these associations.
        value
            .conversations
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"binding_id": 1, "generation": 1, "record.agent_id": 1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .name("binding_generation_agent".to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| StoreError::Unavailable(format!("binding agent index: {error}")))?;
        if let Err(error) = value.conversations.drop_index("record.agent_id_1").await {
            if !matches!(&*error.kind, mongodb::error::ErrorKind::Command(command) if command.code == 27)
            {
                return Err(StoreError::Unavailable(format!(
                    "binding agent index migration: {error}"
                )));
            }
        }
        Ok(value)
    }
}

#[async_trait::async_trait]
impl BindingStore for MongoSandboxStore {
    async fn contains_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError> {
        Ok(self
            .bindings
            .records
            .find_one(doc! {"$or": [{"record.key_sha256": hash}, {"key_history": hash}]})
            .await
            .map_err(driver)?
            .is_some())
    }
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
                doc! {"$set": document, "$addToSet": {"key_history": {"$each": [current.as_ref().expect("validated update").key_sha256.clone(), next.key_sha256.clone()]}}}).await.map_err(driver)?;
            if result.matched_count != 1 {
                return Err(SandboxStoreError::Conflict);
            }
        } else {
            document.insert("key_history", vec![next.key_sha256.clone()]);
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
        let mut query = doc! {"$or": [
            {"pending": true},
            {"record.state.status": "failed", "record.work": "idle",
             "record.previous": null, "record.existing_agent": null,
             "record.profile.environment": {"$ne": null}},
        ]};
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
mod retry_tests;
#[cfg(test)]
mod rotation_tests;

#[cfg(test)]
mod organization_tests;
