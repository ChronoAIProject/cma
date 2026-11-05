//! Preserve legacy CAS, joining an owner fence for scoped transport effects.
use super::super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use super::*;

pub(super) async fn save(
    store: &MongoSandboxStore,
    expected_revision: u64,
    agent: Agent,
) -> Result<(), SandboxStoreError> {
    if agent.revision
        != expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?
    {
        return Err(SandboxStoreError::Conflict);
    }
    let filter = doc! {
        "_id": agent.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(expected_revision, "expected agent revision")
            .map_err(|_| SandboxStoreError::Conflict)?,
        "last_seq": signed(agent.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)?,
        "channel_reuse_cleanup_execution": { "$exists": false },
    "channel_history_seal": { "$exists": false },
        "record.title": record_title_value(&agent),
        "owner": agent.owner.as_str(),
        "record.owner": agent.owner.as_str(),
        "worktree_id": agent.worktree_id.as_str(),
        "record.worktree_id": agent.worktree_id.as_str(),
        "record.work.worktree_recoveries": if agent.work.worktree_recoveries.is_empty() {
            Bson::Null
        } else {
            record_bson(&agent.work.worktree_recoveries).map_err(SandboxStoreError::Corrupt)?
        },
        "record.profile": record_bson(&agent.profile)
            .map_err(SandboxStoreError::Corrupt)?,
        "record.conversation.settings": record_bson(&agent.conversation.settings)
            .map_err(SandboxStoreError::Corrupt)?,
        "record.work.resolved_skills": record_bson(&agent.work.resolved_skills)
            .map_err(SandboxStoreError::Corrupt)?,
        "record.work.live_skills": if agent.work.live_skills.is_empty() { Bson::Null } else {
            record_bson(&agent.work.live_skills).map_err(SandboxStoreError::Corrupt)?
        },
    };
    let fields = agent_replacement_fields(&agent)?;
    let result = update(store, &agent.id, filter, fields).await?;
    if result.matched_count == 1 {
        Ok(())
    } else {
        Err(classify_miss(&store.agents, &agent.id).await?)
    }
}

async fn update(
    store: &MongoSandboxStore,
    id: &AgentId,
    filter: Document,
    fields: Document,
) -> Result<mongodb::results::UpdateResult, SandboxStoreError> {
    let update = doc! {"$set": fields};
    if store.agent_ownership.is_none() || layer2_core::agent_ownership::execution_owner().is_none()
    {
        return store
            .agents
            .update_one(filter, update)
            .await
            .map_err(driver);
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(driver)?;
        start_transaction(&mut session).await.map_err(driver)?;
        store.fence_agent_effect(id, &mut session).await?;
        let result = match store
            .agents
            .update_one(filter.clone(), update.clone())
            .session(&mut session)
            .await
        {
            Ok(result) => result,
            Err(error) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(error) => return Err(driver(error)),
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(driver(error))
            }
        }
    }
    Err(SandboxStoreError::Conflict)
}

fn driver(error: mongodb::error::Error) -> SandboxStoreError {
    if is_duplicate_key(&error) {
        SandboxStoreError::Duplicate
    } else {
        SandboxStoreError::Unavailable(error.to_string())
    }
}
