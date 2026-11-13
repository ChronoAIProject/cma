//! Snapshot transactions for agent-title claims and durable completion.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentTitle, AgentTitleClaim, AgentTitleClaimMode,
    AgentTitleSource, AgentTitleStoreError, SetAgentTitleOutcome, StoredAgentTitle,
};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::{doc, Bson, Document, Timestamp};
use mongodb::options::ReadConcern;

use super::agent_log::{scan_complete_log_with_session, scan_complete_log_with_snapshot_session};
use super::agent_log_integrity::{prepare_append, AppendCapacity, LogSummary, TransactionLogError};
use super::agent_title_claims::current_agent_filter;
use super::agent_title_state::{
    automatic_completion_fields, lease_filter, read_title_state, user_completion_fields,
    TitleState, TITLE,
};
use super::agent_titles::{map_store_error, snapshot_session};
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

enum ApplyError {
    Domain(AgentTitleStoreError),
    Driver(mongodb::error::Error),
}

#[derive(Clone, Copy)]
enum TitleOutcome {
    Automatic(bool),
    User(SetAgentTitleOutcome),
}

enum TitleApply {
    Settled(TitleOutcome),
    Written(Box<TitleReceipt>),
}

#[derive(Clone)]
struct TitleReceipt {
    agent: Agent,
    entry: AgentLogEntry,
    stored: StoredAgentTitle,
    proof: CompletionProof,
    outcome: TitleOutcome,
}

#[derive(Clone)]
enum CompletionProof {
    Automatic(AgentTitleClaim),
    User(String),
}

enum TitleMutation {
    Automatic {
        claim: AgentTitleClaim,
        title: AgentTitle,
        source: AgentTitleSource,
        set_at: String,
    },
    User {
        id: AgentId,
        title: AgentTitle,
        set_at: String,
        operation_token: String,
    },
}

pub(super) async fn complete_title(
    store: &MongoSandboxStore,
    claim: &AgentTitleClaim,
    title: &AgentTitle,
    source: AgentTitleSource,
    set_at: String,
) -> Result<bool, AgentTitleStoreError> {
    if source == AgentTitleSource::User {
        return Err(AgentTitleStoreError::InvalidClaim);
    }
    if seconds_from_rfc3339(&set_at).is_none() {
        return Err(AgentTitleStoreError::InvalidTimestamp);
    }
    match run_title_mutation(
        store,
        TitleMutation::Automatic {
            claim: claim.clone(),
            title: title.clone(),
            source,
            set_at,
        },
    )
    .await?
    {
        TitleOutcome::Automatic(completed) => Ok(completed),
        TitleOutcome::User(_) => unreachable!("automatic mutation returns an automatic outcome"),
    }
}

pub(super) async fn set_user_title(
    store: &MongoSandboxStore,
    id: &AgentId,
    title: &AgentTitle,
    set_at: String,
) -> Result<SetAgentTitleOutcome, AgentTitleStoreError> {
    if seconds_from_rfc3339(&set_at).is_none() {
        return Err(AgentTitleStoreError::InvalidTimestamp);
    }
    let mutation = TitleMutation::User {
        id: id.clone(),
        title: title.clone(),
        set_at,
        operation_token: format!("agent_title_user_{}", uuid::Uuid::new_v4().simple()),
    };
    match run_title_mutation(store, mutation).await? {
        TitleOutcome::User(outcome) => Ok(outcome),
        TitleOutcome::Automatic(_) => unreachable!("user mutation returns a user outcome"),
    }
}

async fn run_title_mutation(
    store: &MongoSandboxStore,
    mutation: TitleMutation,
) -> Result<TitleOutcome, AgentTitleStoreError> {
    for attempt in 0..TRANSACTION_ATTEMPTS {
        if attempt > 0 {
            tokio::task::yield_now().await;
        }
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(_) => return Err(AgentTitleStoreError::Unavailable),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(AgentTitleStoreError::Unavailable);
        }
        let receipt = match apply_title(store, &mutation, &mut session).await {
            Ok(TitleApply::Settled(outcome)) => {
                let _ = session.abort_transaction().await;
                return Ok(outcome);
            }
            Ok(TitleApply::Written(receipt)) => receipt,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(ApplyError::Driver(_)) => {
                let _ = session.abort_transaction().await;
                return Err(AgentTitleStoreError::Unavailable);
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(receipt.outcome),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                let snapshot_time = session
                    .operation_time()
                    .ok_or(AgentTitleStoreError::Unavailable)?;
                let outcome = receipt.outcome;
                let store = store.clone();
                return wait_for_reconciliation("agent title completion", async move {
                    reconcile_title(&store, &receipt, snapshot_time)
                        .await?
                        .then_some(outcome)
                        .ok_or(AgentTitleStoreError::Unavailable)
                })
                .await
                .map_err(|_| AgentTitleStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(_) => return Err(AgentTitleStoreError::Unavailable),
        }
    }
    Err(AgentTitleStoreError::Unavailable)
}

async fn apply_title(
    store: &MongoSandboxStore,
    mutation: &TitleMutation,
    session: &mut mongodb::ClientSession,
) -> Result<TitleApply, ApplyError> {
    let Some((current, state)) = load_agent(store, mutation.id(), session).await? else {
        return match mutation {
            TitleMutation::Automatic { .. } => {
                Ok(TitleApply::Settled(TitleOutcome::Automatic(false)))
            }
            TitleMutation::User { .. } => Err(ApplyError::Domain(AgentTitleStoreError::NotFound)),
        };
    };
    if let Some(outcome) = mutation.settled(&state) {
        return Ok(TitleApply::Settled(outcome));
    }

    let summary = transaction_log(store, &current, session).await?;
    let (next, entry, stored) = next_title_state(&current, mutation)?;

    let mut fields = agent_replacement_fields(&next).map_err(map_domain)?;
    let semantic = summary
        .semantic()
        .cloned()
        .ok_or(ApplyError::Domain(AgentTitleStoreError::Unavailable))?;
    let integrity = prepare_append(
        &current,
        &summary,
        std::slice::from_ref(&entry),
        next.revision,
        semantic
            .after_append(std::slice::from_ref(&entry), &next)
            .map_err(map_domain)?,
        AppendCapacity::Normal,
    )
    .map_err(map_domain)?;
    fields.insert("agent_log_integrity_v1", integrity.integrity);
    fields.insert(
        "last_seq",
        signed(next.conversation.last_seq, "agent log sequence")
            .map_err(|_| ApplyError::Domain(AgentTitleStoreError::SequenceConflict))?,
    );
    fields.extend(mutation.completion_fields());
    let mut filter = current_agent_filter(&current);
    filter.insert(
        TITLE,
        current
            .title
            .clone()
            .map(Bson::String)
            .unwrap_or(Bson::Null),
    );
    filter.insert(
        "record.title",
        current
            .title
            .clone()
            .map(Bson::String)
            .unwrap_or(Bson::Null),
    );
    if let TitleMutation::Automatic { claim, .. } = mutation {
        filter.extend(lease_filter(claim, state.stored.is_some()));
    }
    let updated = store
        .agents
        .update_one(filter, doc! { "$set": fields })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if updated.matched_count != 1 {
        return Err(ApplyError::Domain(AgentTitleStoreError::Unavailable));
    }
    store
        .agent_log
        .clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .insert_one(&integrity.documents[0])
        .session(session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                ApplyError::Domain(AgentTitleStoreError::SequenceConflict)
            } else {
                ApplyError::Driver(error)
            }
        })?;
    let proof = mutation.proof();
    let outcome = mutation.success_outcome();
    Ok(TitleApply::Written(Box::new(TitleReceipt {
        agent: next,
        entry,
        stored,
        proof,
        outcome,
    })))
}

fn next_title_state(
    current: &Agent,
    mutation: &TitleMutation,
) -> Result<(Agent, AgentLogEntry, StoredAgentTitle), ApplyError> {
    let mut next = current.clone();
    next.title = Some(mutation.title().as_str().to_owned());
    next.updated_at = mutation.set_at().to_owned();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(AgentTitleStoreError::Unavailable))?;
    next.conversation.last_seq = next
        .conversation
        .last_seq
        .checked_add(1)
        .ok_or(ApplyError::Domain(AgentTitleStoreError::SequenceConflict))?;
    let stored = StoredAgentTitle {
        agent_id: next.id.clone(),
        title: mutation.title().clone(),
        source: mutation.source(),
        set_at: mutation.set_at().to_owned(),
    };
    let entry = AgentLogEntry {
        agent_id: next.id.clone(),
        seq: next.conversation.last_seq,
        appended_at: mutation.set_at().to_owned(),
        kind: AgentLogKind::TitleSet {
            title: mutation.title().as_str().to_owned(),
            source: mutation.source().as_str().to_owned(),
        },
    };
    Ok((next, entry, stored))
}

async fn load_agent(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<(Agent, TitleState)>, ApplyError> {
    let document = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    let Some(document) = document else {
        return Ok(None);
    };
    let agent = read_agent(&document).map_err(map_domain)?;
    let state = read_title_state(&document, &agent).map_err(ApplyError::Domain)?;
    Ok(Some((agent, state)))
}

mod reconcile;
use reconcile::reconcile_title;

mod mutation;

mod integrity;
use integrity::transaction_log;

mod errors;
use errors::{map_domain, map_reconciliation_log, map_transaction_log};
