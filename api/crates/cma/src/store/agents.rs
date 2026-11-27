//! Mongo persistence for agent aggregates.

use layer2_core::ids::{AgentId, SubjectId};
use layer2_core::sandbox::store::{
    AgentStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::agent_title_state::{initial_title_fields, preserve_title_fields, record_title_value};
use super::sandbox_mongo::{
    i64_field_matches, record_bson, require_version, signed, string_field_matches,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) mod save;

pub(super) const LIVE_STATUSES: &[&str] =
    &["starting", "ready", "working", "waiting_approval", "paused"];

pub(super) fn agent_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "owner": 1, "status": 1, "updated_at": -1 })
            .options(
                IndexOptions::builder()
                    .name("agents_owner_status_updated".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "creation_key": 1 })
            .options(
                IndexOptions::builder()
                    .name("agents_owner_creation_key".to_string())
                    .unique(true)
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("agents_sandbox".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "workspace_id": 1, "status": 1 })
            .options(
                IndexOptions::builder()
                    .name("agents_workspace_status".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {
                "work_pending": 1,
                "work_ready_at": 1,
                "work_lease_expires_at": 1,
                "_id": 1,
            })
            .options(
                IndexOptions::builder()
                    .name("agents_due_work".to_string())
                    .build(),
            )
            .build(),
        // Changed parents can wake a paced retry before its ordinary ready_at.
        IndexModel::builder()
            .keys(
                doc! { "work_pending": 1, "worker_failure_pacing_v1.until": 1,
                "work_lease_expires_at": 1, "_id": 1 },
            )
            .options(
                IndexOptions::builder()
                    .name("agents_paced_work".to_string())
                    .partial_filter_expression(
                        doc! {"worker_failure_pacing_v1.until": {"$exists": true}},
                    )
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "status": 1, "_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("agents_live_connections".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "worktree_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("agents_live_worktree".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "status": { "$in": LIVE_STATUSES } })
                    .build(),
            )
            .build(),
        // Preparation checks both authority and its lookup projection, so a
        // malformed projection cannot hide a live child or force a full scan.
        IndexModel::builder()
            .keys(doc! { "record.sandbox_id": 1 })
            .build(),
    ]
}

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(agent_indexes())
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("agent indexes: {error}")))
}

pub(super) fn agent_updated_at(agent: &Agent) -> &str {
    &agent.updated_at
}

pub(super) fn agent_document(agent: &Agent) -> Result<Document, SandboxStoreError> {
    agent
        .validate_worktree_recoveries()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    agent
        .work
        .validate_resolved_skills(&agent.profile.content.skills)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let lease = agent.work.lease.as_ref();
    let mut document = doc! {
        "_id": agent.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": agent.owner.as_str(),
        "sandbox_id": agent.sandbox_id.as_str(),
        "workspace_id": agent.workspace_id.as_str(),
        "worktree_id": agent.worktree_id.as_str(),
        "creation_key": &agent.creation_key,
        "creation_fingerprint": &agent.creation_fingerprint,
        "status": record_bson(&agent.status).map_err(SandboxStoreError::Corrupt)?,
        "updated_at": agent_updated_at(agent),
        "work_pending": agent.work.step.is_pending(),
        "work_ready_at": &agent.work.ready_at,
        "work_lease_holder": lease
            .map(|lease| Bson::String(lease.holder.clone()))
            .unwrap_or(Bson::Null),
        "work_lease_fence": lease
            .map(|lease| signed(lease.fence, "agent work lease fence"))
            .transpose()
            .map_err(SandboxStoreError::Corrupt)?
            .map(Bson::Int64)
            .unwrap_or(Bson::Null),
        "work_lease_expires_at": lease
            .map(|lease| Bson::String(lease.expires_at.clone()))
            .unwrap_or(Bson::Null),
        "work_next_fence": signed(agent.work.next_fence, "agent work next fence")
            .map_err(SandboxStoreError::Corrupt)?,
        "revision": signed(agent.revision, "agent revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "last_seq": signed(agent.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(agent).map_err(SandboxStoreError::Corrupt)?,
    };
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
    let mut agent: Agent = mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or_else(|| SandboxStoreError::Corrupt("Agent record missing".into()))?,
    )
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    super::agent_log_segments::validate_identity(document, &agent)?;
    super::trigger_first_messages::hydrate(document, &mut agent)?;
    if let Some(timeout) = &agent.conversation.turn_timeout {
        timeout
            .validate()
            .map_err(|_| SandboxStoreError::Corrupt("invalid turn policy".into()))?;
        if !timeout.binding().matches_incarnation(&agent) {
            return Err(SandboxStoreError::Conflict);
        }
    }
    agent
        .validate_worktree_recoveries()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    agent
        .work
        .validate_resolved_skills(&agent.profile.content.skills)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let status = record_bson(&agent.status).map_err(SandboxStoreError::Corrupt)?;
    let revision = signed(agent.revision, "agent revision").map_err(SandboxStoreError::Corrupt)?;
    let last_seq = signed(agent.conversation.last_seq, "agent log sequence")
        .map_err(SandboxStoreError::Corrupt)?;
    let lease = agent.work.lease.as_ref();
    let lease_holder = lease
        .map(|lease| Bson::String(lease.holder.clone()))
        .unwrap_or(Bson::Null);
    let lease_fence = lease
        .map(|lease| signed(lease.fence, "agent work lease fence"))
        .transpose()
        .map_err(SandboxStoreError::Corrupt)?
        .map(Bson::Int64)
        .unwrap_or(Bson::Null);
    let lease_expires_at = lease
        .map(|lease| Bson::String(lease.expires_at.clone()))
        .unwrap_or(Bson::Null);
    let next_fence = signed(agent.work.next_fence, "agent work next fence")
        .map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", agent.id.as_str())
        || !string_field_matches(document, "owner", agent.owner.as_str())
        || !string_field_matches(document, "sandbox_id", agent.sandbox_id.as_str())
        || !string_field_matches(document, "workspace_id", agent.workspace_id.as_str())
        || !string_field_matches(document, "worktree_id", agent.worktree_id.as_str())
        || !string_field_matches(document, "creation_key", &agent.creation_key)
        || !string_field_matches(
            document,
            "creation_fingerprint",
            &agent.creation_fingerprint,
        )
        || document.get("status") != Some(&status)
        || !string_field_matches(document, "updated_at", agent_updated_at(&agent))
        || document.get_bool("work_pending").ok() != Some(agent.work.step.is_pending())
        || !string_field_matches(document, "work_ready_at", &agent.work.ready_at)
        || document.get("work_lease_holder") != Some(&lease_holder)
        || document.get("work_lease_fence") != Some(&lease_fence)
        || document.get("work_lease_expires_at") != Some(&lease_expires_at)
        || !i64_field_matches(document, "work_next_fence", next_fence)
        || !i64_field_matches(document, "revision", revision)
        || !i64_field_matches(document, "last_seq", last_seq)
    {
        return Err(SandboxStoreError::Corrupt(
            "agent query fields disagree with its aggregate".into(),
        ));
    }
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
