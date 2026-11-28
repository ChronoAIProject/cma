//! Core Agent BSON representation and validation of persisted query fields.

use cma_am_mongo_primitives::{i64_field_matches, record_bson, signed, string_field_matches};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{options::IndexOptions, IndexModel};

const DOCUMENT_SCHEMA_VERSION: i64 = 1;

pub const LIVE_STATUSES: &[&str] = &["starting", "ready", "working", "waiting_approval", "paused"];

pub fn agent_indexes() -> Vec<IndexModel> {
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

pub fn agent_updated_at(agent: &Agent) -> &str {
    &agent.updated_at
}

pub fn agent_document(agent: &Agent) -> Result<Document, SandboxStoreError> {
    agent
        .validate_worktree_recoveries()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    agent
        .work
        .validate_resolved_skills(&agent.profile.content.skills)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let lease = agent.work.lease.as_ref();
    Ok(doc! {
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
    })
}

pub fn decode_agent(document: &Document) -> Result<Agent, SandboxStoreError> {
    mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or_else(|| SandboxStoreError::Corrupt("Agent record missing".into()))?,
    )
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

pub fn validate_record(document: &Document, agent: &Agent) -> Result<(), SandboxStoreError> {
    if let Some(timeout) = &agent.conversation.turn_timeout {
        timeout
            .validate()
            .map_err(|_| SandboxStoreError::Corrupt("invalid turn policy".into()))?;
        if !timeout.binding().matches_incarnation(agent) {
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
        || !string_field_matches(document, "updated_at", agent_updated_at(agent))
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
    Ok(())
}
