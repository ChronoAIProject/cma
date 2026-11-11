//! The original Agent save predicates, under the parent/child transaction.

use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{
    bson::{doc, Bson},
    ClientSession,
};

use crate::store::agent_title_state::record_title_value;
use crate::store::agents::{agent_replacement_fields, read_agent};
use crate::store::sandbox_events::TransactionEventError as Error;
use crate::store::sandbox_mongo::{
    record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    expected_revision: u64,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), Error> {
    if Some(agent.revision) != expected_revision.checked_add(1) {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    let current_row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or(Error::Domain(SandboxStoreError::Conflict))?;
    let current = read_agent(&current_row).map_err(Error::Domain)?;
    // Fence the persisted home, never a caller-supplied replacement home.
    if current.revision != expected_revision
        || current.id != agent.id
        || current.owner != agent.owner
        || current.sandbox_id != agent.sandbox_id
        || current.workspace_id != agent.workspace_id
        || current.worktree_id != agent.worktree_id
        || current.created_at != agent.created_at
        || current.creation_key != agent.creation_key
        || current.creation_fingerprint != agent.creation_fingerprint
    {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    let mut fields = agent_replacement_fields(agent).map_err(Error::Domain)?;
    crate::store::agent_log_integrity::carry_integrity_for_revision(
        &current_row,
        &current,
        agent,
        &mut fields,
    )
    .map_err(Error::Domain)?;
    super::fence_parent(store, &current.sandbox_id, &current.owner, session).await?;
    let result = store.agents.update_one(
        doc! {
            "_id": agent.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(expected_revision, "expected agent revision")
                .map_err(|_| Error::Domain(SandboxStoreError::Conflict))?,
            "last_seq": signed(agent.conversation.last_seq, "agent log sequence")
                .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "channel_history_seal": { "$exists": false },
            "record.title": record_title_value(agent),
            "owner": agent.owner.as_str(),
            "record.owner": agent.owner.as_str(),
            "worktree_id": agent.worktree_id.as_str(),
            "record.worktree_id": agent.worktree_id.as_str(),
            "record.work.worktree_recoveries": if agent.work.worktree_recoveries.is_empty() {
                Bson::Null
            } else {
                record_bson(&agent.work.worktree_recoveries)
                    .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?
            },
            "record.profile": record_bson(&agent.profile)
                .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?,
            "record.conversation.settings": record_bson(&agent.conversation.settings)
                .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?,
            "record.work.resolved_skills": record_bson(&agent.work.resolved_skills)
                .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?,
            "record.work.live_skills": if agent.work.live_skills.is_empty() { Bson::Null } else {
                record_bson(&agent.work.live_skills)
                    .map_err(SandboxStoreError::Corrupt).map_err(Error::Domain)?
            },
        },
        doc! {"$set": fields},
    ).session(&mut *session).await.map_err(Error::Driver)?;
    if result.matched_count != 1 {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}
