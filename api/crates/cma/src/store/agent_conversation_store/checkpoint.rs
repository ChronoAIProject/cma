//! Private assistant assembly and exact update receipt on the Agent document.

use layer2_core::sandbox::store::AgentMessageCheckpoint;
use layer2_core::sandbox::AgentUpdateOutcome;
use mongodb::bson::{Bson, Document, Timestamp};

use super::*;
use crate::store::sandbox_mongo::record_bson;

pub(super) const FIELD: &str = "agent_message_checkpoint";

pub(in super::super) fn carry_fields(
    row: &Document,
    current: &Agent,
    next: &Agent,
    fields: &mut Document,
) -> Result<(), SandboxStoreError> {
    if let Some(checkpoint) = read(row)?.and_then(|value| value.advance(current, next)) {
        fields.insert(
            FIELD,
            record_bson(&checkpoint).map_err(SandboxStoreError::Corrupt)?,
        );
    }
    Ok(())
}

pub(super) fn read(row: &Document) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
    let Some(value) = row.get(FIELD) else {
        return Ok(None);
    };
    let checkpoint: AgentMessageCheckpoint = mongodb::bson::deserialize_from_bson(value.clone())
        .map_err(|_| SandboxStoreError::Corrupt("invalid agent message checkpoint".into()))?;
    checkpoint.validate()?;
    Ok(Some(checkpoint))
}

pub(super) async fn load(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
    let Some(row) = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    read_agent(&row)?;
    read(&row)
}

pub(super) async fn carry(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    next: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if let Some(checkpoint) = read(row).map_err(ApplyError::Domain)? {
        let carried = checkpoint.advance(current, next);
        write(store, next, carried.as_ref(), session).await?;
    }
    Ok(())
}

pub(super) enum Authority {
    Live,
    Reopen(Box<layer2_core::sandbox::store::AgentReopenGuard>),
    Capacity(Box<layer2_core::sandbox::store::AgentConversationCapacitySettlement>),
}

pub(super) async fn commit(
    store: &MongoSandboxStore,
    current: Agent,
    previous: Option<AgentMessageCheckpoint>,
    next: AgentMessageCheckpoint,
    mutation: Option<AgentConversationMutation>,
    authority: Authority,
) -> Result<AgentUpdateOutcome, SandboxStoreError> {
    next.validate()?;
    let _writer = store.agent_writes.lock(&current.id).await;
    let _platform = match &mutation {
        Some(mutation) => store.conversation_platform_guard(mutation).await,
        None => None,
    };
    let reanchor = mutation.is_none()
        && previous
            .as_ref()
            .and_then(|value| value.reanchored(&current))
            .as_ref()
            == Some(&next);
    if let Some(mutation) = &mutation {
        ensure_append_batch_size(mutation.agent_logs.len())?;
    }
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        let (outcome, published) = match apply(
            store,
            &current,
            previous.as_ref(),
            &next,
            mutation.as_ref(),
            &authority,
            &mut session,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                if published {
                    crate::telemetry::work::published(&outcome.appended, "committed");
                }
                return Ok(outcome);
            }
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "agent message commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved = commit_error.clone();
                let store = store.clone();
                let capacity = match &authority {
                    Authority::Capacity(proof) => Some(proof.clone()),
                    _ => None,
                };
                return wait_for_reconciliation("agent message commit", async move {
                    let confirmed = reconcile(
                        &store,
                        &current.id,
                        &next,
                        snapshot_time,
                        reanchor,
                        capacity.as_deref(),
                    )
                    .await?
                    .ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent message commit remained unknown: {unresolved}"
                        ))
                    })?;
                    if published {
                        crate::telemetry::work::published(&confirmed.appended, "reconciled");
                    }
                    Ok(confirmed)
                })
                .await
                .map_err(|deadline| {
                    SandboxStoreError::Unavailable(format!(
                        "{deadline}; ambiguous commit: {commit_error}"
                    ))
                })?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()))
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "agent message transaction exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn apply(
    store: &MongoSandboxStore,
    current: &Agent,
    previous: Option<&AgentMessageCheckpoint>,
    next: &AgentMessageCheckpoint,
    mutation: Option<&AgentConversationMutation>,
    authority: &Authority,
    session: &mut mongodb::ClientSession,
) -> Result<(AgentUpdateOutcome, bool), ApplyError> {
    let guard = match authority {
        Authority::Reopen(guard) => Some(guard.as_ref()),
        _ => None,
    };
    let capacity = match authority {
        Authority::Capacity(proof) => Some(proof.as_ref()),
        _ => None,
    };
    let row = store
        .agents
        .find_one(doc! { "_id": current.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let actual_agent = read_agent(&row).map_err(ApplyError::Domain)?;
    let actual = read(&row).map_err(ApplyError::Domain)?;
    if let Some(guard) = guard {
        if actual_agent != *current {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        super::reopen::validate_and_fence(store, &actual_agent, guard, session).await?;
    } else if actual.as_ref().is_some_and(|checkpoint| {
        Some(checkpoint.runtime_generation) > current.conversation.runtime_generation
    }) {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let project = |agent: &Agent| match guard {
        Some(guard) => guard.project(agent).map_err(ApplyError::Domain),
        None => Ok(agent.clone()),
    };
    if mutation.is_none()
        && previous
            .and_then(|value| value.reanchored(current))
            .as_ref()
            == Some(next)
    {
        if let Some(outcome) =
            replay_at_anchor(actual.as_ref(), next, true).map_err(ApplyError::Domain)?
        {
            return Ok((outcome, false));
        }
        if actual_agent != *current || actual.as_ref() != previous || !current.status.is_live() {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        write(store, current, Some(next), session).await?;
        return Ok((next.outcome.clone(), false));
    }
    if let Some(outcome) = replay(actual.as_ref(), next).map_err(ApplyError::Domain)? {
        return Ok((outcome, false));
    }
    if actual_agent != *current || actual.as_ref() != previous || !current.status.is_live() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let updated = if let Some(mutation) = mutation {
        if mutation.agent.id != current.id || !next.matches(&project(&mutation.agent)?) {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        if let Some(proof) = capacity {
            proof
                .validate_snapshot(&actual_agent, actual.as_ref())
                .map_err(ApplyError::Domain)?;
        }
        let receipt = apply_mutation_with_capacity(
            ScanCaller::Checkpoint,
            store,
            mutation,
            session,
            capacity,
        )
        .await?;
        if receipt.logs != next.outcome.appended {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        &mutation.agent
    } else {
        if !next.matches(&project(current)?)
            || !next.outcome.appended.is_empty()
            || next.outcome.turn_closed
        {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        current
    };
    write(store, updated, Some(next), session).await?;
    if let Some(proof) = capacity {
        // Replace the active checkpoint with the compact receipt before adding
        // the retained prefix, avoiding two large checkpoints in one document.
        // Both writes remain part of this same transaction.
        capacity_prefix::preserve(store, &row, proof, session).await?;
    }
    if let Some(mutation) = mutation {
        admission::require_current(mutation, session)
            .await
            .map_err(ApplyError::Domain)?;
    }
    Ok((next.outcome.clone(), mutation.is_some()))
}

fn replay(
    actual: Option<&AgentMessageCheckpoint>,
    next: &AgentMessageCheckpoint,
) -> Result<Option<AgentUpdateOutcome>, SandboxStoreError> {
    match actual.filter(|value| value.update_id == next.update_id) {
        Some(recorded) if recorded.update_fingerprint == next.update_fingerprint => {
            Ok(Some(recorded.outcome.clone()))
        }
        Some(_) => Err(SandboxStoreError::Conflict),
        None => Ok(None),
    }
}

pub(super) fn replay_at_anchor(
    actual: Option<&AgentMessageCheckpoint>,
    next: &AgentMessageCheckpoint,
    require_anchor: bool,
) -> Result<Option<AgentUpdateOutcome>, SandboxStoreError> {
    if require_anchor && actual.is_none_or(|value| value.after_seq < next.after_seq) {
        return Ok(None);
    }
    replay(actual, next)
}

async fn write(
    store: &MongoSandboxStore,
    current: &Agent,
    checkpoint: Option<&AgentMessageCheckpoint>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let revision = signed(current.revision, "agent revision")
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let last_seq = signed(current.conversation.last_seq, "agent log sequence")
        .map_err(SandboxStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let update = match checkpoint {
        Some(checkpoint) => {
            checkpoint.validate().map_err(ApplyError::Domain)?;
            let value: Bson = record_bson(checkpoint)
                .map_err(SandboxStoreError::Corrupt)
                .map_err(ApplyError::Domain)?;
            doc! { "$set": { FIELD: value } }
        }
        None => doc! { "$unset": { FIELD: "" } },
    };
    let result = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": revision,
                "last_seq": last_seq,
                "record.conversation.last_seq": last_seq,
                "channel_history_seal": { "$exists": false },
            },
            update,
        )
        .session(session)
        .await
        .map_err(classify_write)?;
    if result.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

async fn reconcile(
    store: &MongoSandboxStore,
    id: &AgentId,
    next: &AgentMessageCheckpoint,
    snapshot_time: Timestamp,
    require_anchor: bool,
    capacity: Option<&layer2_core::sandbox::store::AgentConversationCapacitySettlement>,
) -> Result<Option<AgentUpdateOutcome>, SandboxStoreError> {
    let mut session = snapshot_session(store, Some(snapshot_time)).await?;
    let row = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    read_agent(&row)?;
    let actual = read(&row)?;
    if let Some(proof) = capacity {
        let receipt = ConversationReceipt {
            command_dispatch: None,
            agent: proof.mutation().agent.clone(),
            logs: proof.checkpoint().outcome.appended.clone(),
        };
        if actual.as_ref() != Some(proof.checkpoint())
            || !capacity_prefix::matches(&row, proof)?
            || !reconcile_conversation_snapshot(store, &receipt, &mut session).await?
        {
            return Ok(None);
        }
    }
    replay_at_anchor(actual.as_ref(), next, require_anchor)
}
