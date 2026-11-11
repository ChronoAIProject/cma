//! Keep the legacy path explicit while segmented consumers require S1 proof.
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        capture::{self, CapturedLog},
        read_format,
        transaction_scope::TransactionScope,
    },
    agents::read_agent,
    sandbox_mongo::{signed, MongoSandboxStore},
};

/// Dispatch hint only. The selected transaction still validates every envelope;
/// concurrent adoption makes the legacy path refuse, never read a v2 lifetime.
pub(in crate::store) async fn sandbox_uses_segments(
    store: &MongoSandboxStore,
    id: &layer2_core::SandboxId,
) -> Result<bool, TransactionLogError> {
    store
        .agents
        .find_one(doc! {
            "sandbox_id": id.as_str(),
            "$or": [
                {"agent_log_format": {"$exists": true}},
                {"document_schema_version": {"$ne": 1_i64}},
            ],
        })
        .await
        .map(|row| row.is_some())
        .map_err(TransactionLogError::Driver)
}

/// The caller already owns a transaction. A segmented capture failure is never
/// converted to None: only a positively identified legacy envelope selects the
/// unchanged legacy validator. The same session binds envelope and aggregate.
pub(in crate::store) async fn capture_segmented(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<CapturedLog>, TransactionLogError> {
    let row = current_row(store, agent, session).await?;
    if read_format(&row)?.is_none() {
        return Ok(None);
    }
    // A raw session cannot establish a transaction stamp, even if its caller
    // already started a transaction. Only the scoped entry can capture v2.
    Err(SandboxStoreError::Unavailable(
        "segmented recovery consumer requires an exclusive transaction scope".into(),
    )
    .into())
}

pub(in crate::store) async fn capture_segmented_scoped(
    store: &MongoSandboxStore,
    agent: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<CapturedLog>, TransactionLogError> {
    let row = current_row_scoped(store, agent, scope).await?;
    if read_format(&row)?.is_none() {
        return Ok(None);
    }
    capture::capture_transaction(store, agent, scope)
        .await
        .map(Some)
}

async fn current_row(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Document, TransactionLogError> {
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    validate_row(agent, row)
}

async fn current_row_scoped(
    store: &MongoSandboxStore,
    agent: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<Document, TransactionLogError> {
    let row = scope
        .find_one(&store.agents, doc! {"_id": agent.id.as_str()})
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    validate_row(agent, row)
}

fn validate_row(agent: &Agent, row: Document) -> Result<Document, TransactionLogError> {
    if read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    read_format(&row)?;
    Ok(row)
}

/// Exact envelope fences support both formats; a blanket schema-v1 predicate
/// would refuse every segmented Agent after a successful capture.
pub(in crate::store) async fn fence_filter(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Document, TransactionLogError> {
    let row = current_row(store, agent, session).await?;
    envelope_filter(agent, &row)
}

pub(in crate::store) async fn fence_filter_scoped(
    store: &MongoSandboxStore,
    agent: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<Document, TransactionLogError> {
    let row = current_row_scoped(store, agent, scope).await?;
    envelope_filter(agent, &row)
}

fn envelope_filter(agent: &Agent, row: &Document) -> Result<Document, TransactionLogError> {
    let number = |value, name| signed(value, name).map_err(SandboxStoreError::Corrupt);
    let mut filter = doc! {
        "_id": agent.id.as_str(), "owner": agent.owner.as_str(),
        "sandbox_id": agent.sandbox_id.as_str(),
        "revision": number(agent.revision, "recovery Agent revision")?,
        "last_seq": number(agent.conversation.last_seq, "recovery Agent head")?,
    };
    for key in [
        "document_schema_version",
        "agent_log_format",
        "agent_log_incarnation",
        "agent_log_head_revision",
    ] {
        match row.get(key) {
            Some(value) => {
                filter.insert(key, value.clone());
            }
            None => {
                filter.insert(key, doc! {"$exists": false});
            }
        }
    }
    Ok(filter)
}

pub(in crate::store) async fn fence_agent(
    store: &MongoSandboxStore,
    agent: &Agent,
    field: &str,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let filter = fence_filter(store, agent, session).await?;
    let result = store
        .agents
        .update_one(filter, doc! {"$inc": {field: 1_i64}})
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(())
}
