//! Mongo persistence for agent aggregates.

use layer2_core::ids::{AgentId, SubjectId};
use layer2_core::sandbox::store::{
    AgentStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Document};
use mongodb::Collection;

use super::agent_title_state::{initial_title_fields, preserve_title_fields, record_title_value};
use super::sandbox_mongo::{
    record_bson, require_version, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) mod save;

use cma_am_sandbox_state::agent::agent_indexes;
pub(super) use cma_am_sandbox_state::agent::LIVE_STATUSES;

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(agent_indexes())
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("agent indexes: {error}")))
}

pub(super) fn agent_document(agent: &Agent) -> Result<Document, SandboxStoreError> {
    let mut document = cma_am_sandbox_state::agent::agent_document(agent)?;
    if let Some(binding) = agent
        .conversation
        .first_message
        .as_ref()
        .and_then(|first| first.common_binding.as_ref())
    {
        document.insert(
            super::trigger_first_messages::FIELD,
            record_bson(binding).map_err(SandboxStoreError::Corrupt)?,
        );
    }
    document.extend(initial_title_fields(agent)?);
    Ok(document)
}

pub(super) fn read_agent(document: &Document) -> Result<Agent, SandboxStoreError> {
    super::agent_log_segments::require_agent_version(document)
        .map_err(SandboxStoreError::Corrupt)?;
    let mut agent = cma_am_sandbox_state::agent::decode_agent(document)?;
    super::agent_log_segments::validate_identity(document, &agent)?;
    super::trigger_first_messages::hydrate(document, &mut agent)?;
    cma_am_sandbox_state::agent::validate_record(document, &agent)?;
    Ok(agent)
}

pub(super) fn agent_replacement_fields(agent: &Agent) -> Result<Document, SandboxStoreError> {
    let mut document = agent_document(agent)?;
    document.remove("_id");
    document.remove("last_seq");
    // Revision-only replacements must retain the durable format discriminator.
    document.remove("document_schema_version");
    preserve_title_fields(&mut document);
    Ok(document)
}

async fn classify_miss(
    collection: &Collection<Document>,
    id: &AgentId,
) -> Result<SandboxStoreError, SandboxStoreError> {
    match collection
        .find_one(doc! { "_id": id.as_str() })
        .projection(doc! { "document_schema_version": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        None => Ok(SandboxStoreError::NotFound),
        Some(document) => {
            require_version(&document).map_err(SandboxStoreError::Corrupt)?;
            Ok(SandboxStoreError::Conflict)
        }
    }
}

#[async_trait::async_trait]
impl AgentStore for MongoSandboxStore {
    async fn trigger_ready_for_claim(
        &self,
        command: &layer2_core::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<(), SandboxStoreError> {
        super::trigger_first_messages::ready_for_claim(self, command).await
    }

    async fn trigger_execution_proof(
        &self,
        command: &layer2_core::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<layer2_core::sandbox::agent_io::TriggerExecutionProof, SandboxStoreError> {
        super::trigger_first_messages::proof(self, command).await
    }
    async fn require_legacy_first_message(&self, agent: &Agent) -> Result<(), SandboxStoreError> {
        super::trigger_first_messages::legacy(self, agent).await
    }

    async fn create_agent(&self, agent: Agent) -> Result<(), SandboxStoreError> {
        if agent.revision != 1
            || agent.work.resolved_skills.is_some()
            || !agent.work.live_skills.is_empty()
        {
            return Err(SandboxStoreError::Conflict);
        }
        if agent.conversation.last_seq != 0 {
            return Err(SandboxStoreError::SequenceConflict);
        }
        // Creation provenance survives physical Sandbox purge. Its unique Agent
        // identity also fences recreation through this lower-level creation port.
        if self
            .agent_creation_provenance
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .map_err(|_| SandboxStoreError::Unavailable("Agent identity check unavailable".into()))?
            .is_some()
        {
            return Err(SandboxStoreError::Duplicate);
        }
        super::native_retirement_action::insert_child(
            self,
            &self.agents,
            &agent.sandbox_id,
            agent_document(&agent)?,
        )
        .await
    }

    async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>, SandboxStoreError> {
        let agent = self
            .agents
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_agent)
            .transpose()?;
        if let Some(agent) = &agent {
            layer2_core::agent_ownership::receiver_diagnostics::observe_agent_sandbox(
                &agent.id,
                &agent.sandbox_id,
            );
        }
        Ok(agent)
    }

    async fn save_agent(
        &self,
        expected_revision: u64,
        agent: Agent,
    ) -> Result<(), SandboxStoreError> {
        save::save(self, expected_revision, agent).await
    }

    async fn list_agents(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "owner": owner.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut read = super::mongo_read_telemetry::Read::find(self.agents.name());
        let mut cursor = self
            .agents
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = read
            .next(&mut cursor)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_agent(&document)?);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded page has a last item")
                .id
                .0
                .clone()
        });
        read.complete();
        Ok(Page { items, next })
    }
}

#[cfg(test)]
mod tests;
