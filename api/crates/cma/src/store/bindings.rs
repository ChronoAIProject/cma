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
    Collection, Database,
};

use super::durability::majority_collection;
use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore};
use super::schema::{
    verify_existing_collections, Contract, StartupObserver, DOCUMENT_SCHEMA_VERSION,
};
#[cfg(test)]
use mongodb::IndexModel;
mod admission;
mod associations;
mod commands;
mod conversations;
mod custody;
mod key_classification;
mod profile_transfer;
mod reprovision;
mod reset;
#[cfg(test)]
mod reset_reader_tests;
#[cfg(test)]
mod reset_tests;
mod retry;
mod rotation;
pub(in crate::store) mod schema;
mod tools;
pub(crate) use custody::MongoBindingCredentials;
pub(in crate::store) use retry::BindingTransactionError;
pub(crate) use tools::MongoProviderToolSource;
mod documents;
use documents::*;
#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod association_tests;
#[cfg(test)]
mod ceiling_tests;
#[cfg(test)]
mod existing_tests;
#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
pub(super) use mongo_tests::insert_retained_binding_fixture;
#[cfg(test)]
mod operator_stop_tests;
#[cfg(test)]
mod schema_tests;

const BINDINGS: &str = "provider_bindings";
const CONVERSATIONS: &str = "binding_conversations";
const TOOL_OPENS: &str = "provider_tool_opens";
const TOOL_RELAYS: &str = "provider_tool_relays";

#[derive(Clone)]
pub(super) struct BindingCollections {
    pub(super) safety: schema::BindingSafety,
    pub(super) records: Collection<Document>,
    pub(super) conversations: Collection<Document>,
    pub(super) tool_opens: Collection<Document>,
    pub(super) tool_relays: Collection<Document>,
    pub(super) preparations: Collection<Document>,
}

impl BindingCollections {
    pub(super) async fn open(
        database: &Database,
        observer: &mut StartupObserver,
    ) -> Result<Self, StoreError> {
        verify_existing_collections(database, schema::COLLECTIONS).await?;
        let assessment = observer
            .assess(Contract::Binding, schema::inspect(database))
            .await?;
        Ok(Self::handles(database, assessment.capability))
    }

    pub(in crate::store) fn handles(database: &Database, safety: schema::BindingSafety) -> Self {
        Self {
            safety,
            records: majority_collection(database, BINDINGS),
            conversations: majority_collection(database, CONVERSATIONS),
            tool_opens: majority_collection(database, TOOL_OPENS),
            tool_relays: majority_collection(database, TOOL_RELAYS),
            preparations: majority_collection(database, reprovision::PREPARATIONS),
        }
    }
}

#[async_trait::async_trait]
impl BindingStore for MongoSandboxStore {
    async fn transfer_profile_conversation(
        &self,
        request: layer2_core::sandbox::binding::ProfileTransferRequest,
        enabled: bool,
    ) -> Result<
        layer2_core::sandbox::binding::ProfileTransferReceipt,
        layer2_core::sandbox::binding::BindingError,
    > {
        self.transfer_conversation(request, enabled).await
    }

    async fn conversation_reset_state(
        &self,
        binding: &str,
    ) -> Result<layer2_core::sandbox::binding::ConversationResetState, SandboxStoreError> {
        let row = self
            .bindings
            .records
            .find_one(doc! {"_id":binding})
            .await
            .map_err(driver)?
            .ok_or(SandboxStoreError::NotFound)?;
        reset::read_state(&row)
    }
    async fn reset_binding_conversation(
        &self,
        request: layer2_core::sandbox::binding::ConversationResetRequest,
        enabled: bool,
    ) -> Result<
        layer2_core::sandbox::binding::ConversationResetReceipt,
        layer2_core::sandbox::binding::BindingError,
    > {
        self.reset_conversation(request, enabled).await
    }
    async fn contains_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError> {
        Ok(self
            .bindings
            .records
            .find_one(doc! {"$or": [{"record.key_sha256": hash}, {"key_history": hash}]})
            .await
            .map_err(driver)?
            .is_some())
    }
    async fn classify_worker_key_hash(&self, hash: &str) -> Result<bool, SandboxStoreError> {
        key_classification::contains(self, hash).await
    }
    async fn find_by_key_hash(
        &self,
        hash: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        self.bindings.safety.require_keys()?;
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
        self.bindings.safety.require_associations()?;
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
        if current
            .as_ref()
            .is_none_or(|old| old.key_sha256 != next.key_sha256)
        {
            self.bindings.safety.require_keys()?;
        }
        validate_binding_write(current.as_ref(), expected, &next)?;
        if next.existing_agent.is_none()
            && current
                .as_ref()
                .is_none_or(|old| old.generation != next.generation)
        {
            let creates_home = next.profile.environment.is_some()
                && current
                    .as_ref()
                    .is_none_or(|old| old.sandbox_id != next.sandbox_id);
            crate::store::admission_checks::require_fresh_home_read(
                &self.budget_settings,
                creates_home,
            )
            .await?;
        }
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
        if self
            .empty_discovery
            .empty(
                super::empty_discovery::Kind::Bindings,
                &self.bindings.records,
                query.clone(),
            )
            .await
        {
            return Ok(Vec::new());
        }
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
        let items: Vec<_> = docs
            .into_iter()
            .map(read_binding)
            .collect::<Result<_, _>>()?;
        self.empty_discovery
            .observed(super::empty_discovery::Kind::Bindings, items.is_empty());
        Ok(items)
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
        if self
            .empty_discovery
            .empty(
                super::empty_discovery::Kind::Conversations,
                &self.bindings.conversations,
                query.clone(),
            )
            .await
        {
            return Ok(Vec::new());
        }
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
        let items: Vec<_> = docs
            .into_iter()
            .map(read_conversation)
            .collect::<Result<_, _>>()?;
        self.empty_discovery.observed(
            super::empty_discovery::Kind::Conversations,
            items.is_empty(),
        );
        Ok(items)
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
