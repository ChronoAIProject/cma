//! Preserve legacy CAS, joining an owner fence for scoped transport effects.
use super::super::native_retirement_action;
use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use super::*;
use crate::store::agent_ownership::FenceError;
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::SandboxId;

pub(super) async fn save(
    store: &MongoSandboxStore,
    expected_revision: u64,
    agent: Agent,
) -> Result<(), SandboxStoreError> {
    save_with_format(store, expected_revision, agent, None).await
}

/// Connection recovery changes only the durable work schedule. Its exact
/// predecessor can be either a legacy or an adopted Agent.
pub(in crate::store) async fn save_connection_recovery(
    store: &MongoSandboxStore,
    current: &Agent,
    next: Agent,
) -> Result<(), SandboxStoreError> {
    if current.conversation != next.conversation
        || (store.agent_ownership.is_some()
            && layer2_core::agent_ownership::execution_owner().is_none())
    {
        return Err(SandboxStoreError::Conflict);
    }
    save_with_format(store, current.revision, next, Some(current)).await
}

async fn save_with_format(
    store: &MongoSandboxStore,
    expected_revision: u64,
    agent: Agent,
    recovery_source: Option<&Agent>,
) -> Result<(), SandboxStoreError> {
    if agent.revision
        != expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut filter = doc! {
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
        "sandbox_id": agent.sandbox_id.as_str(),
        "record.sandbox_id": agent.sandbox_id.as_str(),
        "workspace_id": agent.workspace_id.as_str(),
        "record.workspace_id": agent.workspace_id.as_str(),
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
    let current_row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::Conflict)?;
    let current = read_agent(&current_row)?;
    if let Some(source) = recovery_source {
        let bound =
            super::super::agent_capacity_recovery::source::envelope_filter(source, &current_row)?;
        filter.extend(bound);
        filter.insert(
            "record.revision",
            signed(source.revision, "recovery Agent revision")
                .map_err(SandboxStoreError::Corrupt)?,
        );
        filter.insert(
            "record.conversation.last_seq",
            signed(source.conversation.last_seq, "recovery Agent head")
                .map_err(SandboxStoreError::Corrupt)?,
        );
    }
    if current.conversation.turn_timeout != agent.conversation.turn_timeout {
        return Err(SandboxStoreError::Conflict);
    }
    let mut fields = agent_replacement_fields(&agent)?;
    super::super::agent_log_integrity::carry_integrity_for_revision(
        &current_row,
        &current,
        &agent,
        &mut fields,
    )?;
    let new_obligation =
        native_retirement_action::adds_obligation(&current, &agent).then_some(&current.sandbox_id);
    let result = update(store, &agent.id, filter, fields, new_obligation).await?;
    if result.matched_count == 1 {
        Ok(())
    } else if recovery_source.is_some() {
        Err(SandboxStoreError::Conflict)
    } else {
        Err(classify_miss(&store.agents, &agent.id).await?)
    }
}

async fn update(
    store: &MongoSandboxStore,
    id: &AgentId,
    filter: Document,
    fields: Document,
    new_obligation: Option<&SandboxId>,
) -> Result<mongodb::results::UpdateResult, SandboxStoreError> {
    let update = doc! {"$set": fields};
    let owner_scoped = store.agent_ownership.is_some()
        && layer2_core::agent_ownership::execution_owner().is_some();
    if new_obligation.is_none() && !owner_scoped {
        return store
            .agents
            .update_one(filter, update)
            .await
            .map_err(driver);
    }
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if native_retirement_action::retryable_body_error(&error) => continue,
            Err(error) => return Err(driver(error)),
        };
        if let Err(error) = start_transaction(&mut session).await {
            let _ = session.abort_guarded().await;
            if native_retirement_action::retryable_body_error(&error) {
                continue;
            }
            return Err(driver(error));
        }
        if let Err(error) = store.fence_agent_effect_transaction(id, &mut session).await {
            let _ = session.abort_guarded().await;
            match error {
                FenceError::Driver(error)
                    if native_retirement_action::retryable_body_error(&error) =>
                {
                    continue
                }
                FenceError::Driver(error) => return Err(driver(error)),
                FenceError::Domain(error) => return Err(error),
            }
        }
        if let Some(sandbox) = new_obligation {
            match native_retirement_action::fence_obligation(store, sandbox, &mut session).await {
                Ok(true) => {}
                Ok(false) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Conflict);
                }
                Err(error) if native_retirement_action::retryable_body_error(&error) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                Err(error) => return Err(driver(error)),
            }
        }
        let result = match store
            .agents
            .update_one(filter.clone(), update.clone())
            .session(&mut session)
            .await
        {
            Ok(result) => result,
            Err(error) if native_retirement_action::retryable_body_error(&error) => {
                let _ = session.abort_guarded().await;
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
