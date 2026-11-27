//! Private assistant assembly and exact update receipt on the Agent document.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::AgentMessageCheckpoint;
use layer2_core::sandbox::AgentUpdateOutcome;
use mongodb::bson::{Bson, Document, Timestamp};

use super::*;
use crate::store::sandbox_mongo::record_bson;

pub(in crate::store) const FIELD: &str = "agent_message_checkpoint";

pub(in crate::store) fn set_field(
    fields: &mut Document,
    value: &AgentMessageCheckpoint,
) -> Result<(), SandboxStoreError> {
    value.validate()?;
    let encoded = record_bson(value).map_err(SandboxStoreError::Corrupt)?;
    fields.insert(FIELD, encoded);
    Ok(())
}

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

pub(in crate::store) fn read(
    row: &Document,
) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
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
    Ok(load_snapshot(store, id)
        .await?
        .and_then(|snapshot| snapshot.checkpoint))
}

pub(super) async fn load_snapshot(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<Option<layer2_core::sandbox::store::AgentMessageSnapshot>, SandboxStoreError> {
    store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(|row| {
            Ok(layer2_core::sandbox::store::AgentMessageSnapshot {
                agent: read_agent(row)?,
                checkpoint: read(row)?,
            })
        })
        .transpose()
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

pub(in crate::store) enum Authority {
    Live,
    StaleTurn,
    TurnPolicy,
    Reopen(Box<layer2_core::sandbox::store::AgentReopenGuard>),
    Capacity(Box<layer2_core::sandbox::store::AgentConversationCapacitySettlement>),
}

// Allocate the transaction future before the caller starts polling it. Boxing
// inside an async caller still reserves its construction temporary on that stack.
pub(super) fn commit(
    store: &MongoSandboxStore,
    current: Agent,
    previous: Option<AgentMessageCheckpoint>,
    next: AgentMessageCheckpoint,
    mutation: Option<AgentConversationMutation>,
    authority: Authority,
) -> impl std::future::Future<Output = Result<AgentUpdateOutcome, SandboxStoreError>> + '_ {
    Box::pin(commit_inner(
        store, current, previous, next, mutation, authority,
    ))
}

async fn commit_inner(
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
    let adoption = match (&authority, mutation.as_ref()) {
        (Authority::Live, Some(mutation)) => {
            Box::pin(scoped::adoption::prepare(store, mutation, Some(&next))).await?
        }
        _ => None,
    };
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        contention::backoff(attempt).await;
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let mut scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error.to_string());
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        let applied = Box::pin(scoped::checkpoint::apply(
            store,
            (&current, adoption.as_ref()),
            previous.as_ref(),
            &next,
            mutation.as_ref(),
            &authority,
            &mut scope,
        ))
        .await;
        scope.release();
        let (outcome, publication) = match applied {
            Ok(outcome) => outcome,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        if let Some(mutation) = &mutation {
            admission::require_current(mutation, &mut session).await?;
        }
        if let Err(error) = store
            .fence_agent_effect_transaction(&current.id, &mut session)
            .await
        {
            let _ = session.abort_guarded().await;
            match error {
                crate::store::agent_ownership::FenceError::Domain(error) => return Err(error),
                crate::store::agent_ownership::FenceError::Driver(error)
                    if is_transient(&error) =>
                {
                    last_transient = Some("checkpoint ownership fence aborted".into());
                    continue;
                }
                crate::store::agent_ownership::FenceError::Driver(_) => {
                    return Err(SandboxStoreError::Unavailable(
                        "checkpoint ownership fence unavailable".into(),
                    ));
                }
            }
        }
        let published = publication.is_some();
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                if published {
                    publish_work(store, &outcome.appended, "committed");
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
                    let confirmed = Box::pin(reconcile(
                        &store,
                        &current.id,
                        &next,
                        snapshot_time,
                        reanchor,
                        capacity.as_deref(),
                        publication.as_ref(),
                    ))
                    .await?
                    .ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent message commit remained unknown: {unresolved}"
                        ))
                    })?;
                    if published {
                        publish_work(&store, &confirmed.appended, "reconciled");
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
    // Every attempt here failed before commit or was definitively aborted.
    // Let the recorder refresh its snapshot; Unavailable would retain an
    // ambiguous receipt that can never follow a later checkpoint advance.
    // Unknown commit outcomes return through reconciliation above instead.
    Err(contention::exhausted(last_transient))
}

pub(in crate::store) fn replay(
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

pub(in crate::store) fn replay_at_anchor(
    actual: Option<&AgentMessageCheckpoint>,
    next: &AgentMessageCheckpoint,
    require_anchor: bool,
) -> Result<Option<AgentUpdateOutcome>, SandboxStoreError> {
    if require_anchor && actual.is_none_or(|value| value.after_seq < next.after_seq) {
        return Ok(None);
    }
    replay(actual, next)
}

pub(in crate::store) async fn write(
    store: &MongoSandboxStore,
    current: &Agent,
    checkpoint: Option<&AgentMessageCheckpoint>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    write_from_row(store, current, &row, checkpoint, session).await
}

/// Only use a row read in this transaction whose aggregate has not been mutated.
/// Publication may change the format/head as well as revision and last_seq.
pub(in crate::store) async fn write_from_row(
    store: &MongoSandboxStore,
    current: &Agent,
    row: &Document,
    checkpoint: Option<&AgentMessageCheckpoint>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    super::super::agent_log_segments::ordinary::source(store, current, row, session).await?;
    let mut filter = super::super::agent_capacity_recovery::source::envelope_filter(current, row)
        .map_err(ApplyError::Domain)?;
    filter.insert(
        "record.conversation.last_seq",
        signed(current.conversation.last_seq, "checkpoint sequence")
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
    );
    filter.insert("channel_history_seal", doc! {"$exists":false});
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
        .update_one(filter, update)
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
    publication: Option<&ConversationReceipt>,
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
    if super::super::agent_log_segments::read_format(&row)?.is_some()
        && actual.as_ref() != Some(next)
    {
        return Ok(None);
    }
    if let Some(receipt) = publication {
        if actual.as_ref() != Some(next)
            || !reconcile_conversation_snapshot(store, receipt, &mut session).await?
        {
            return Ok(None);
        }
    }
    if let Some(proof) = capacity {
        let receipt = ConversationReceipt {
            segmented: None,
            command_dispatch: None,
            agent: proof.mutation().agent.clone(),
            logs: proof.checkpoint().outcome.appended.clone(),
        };
        if actual.as_ref() != Some(proof.checkpoint())
            || !capacity_prefix::matches(&row, proof)?
            || (publication.is_none()
                && !reconcile_conversation_snapshot(store, &receipt, &mut session).await?)
        {
            return Ok(None);
        }
    }
    replay_at_anchor(actual.as_ref(), next, require_anchor)
}
