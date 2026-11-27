//! Resume a bounded structural audit of an otherwise dormant legacy Agent.
use super::*;
use crate::store::{
    agent_capacity_recovery::source::envelope_filter,
    agent_log_segments::read_format,
    agents::read_agent,
    connection_recovery_activity,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use layer2_core::sandbox::SandboxAgentLogActivity;
use mongodb::{options::Hint, ClientSession};
use serde::{Deserialize, Serialize};

mod tip;

const FIELD: &str = "inactivity_log_proof_v1";
const PAGE_ROWS: i64 = 128;
const PAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum Preparation {
    Pending,
    Certified,
    ByteLimit,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    revision: u64,
    head: u64,
    lineage: [u8; HASH_BYTES],
    through: u64,
    bytes: usize,
    tail_hash: [u8; HASH_BYTES],
    state: Preparation,
    integrity: Option<[u8; HASH_BYTES]>,
}

impl Progress {
    fn start(agent: &Agent, integrity: Option<[u8; HASH_BYTES]>) -> Self {
        Self {
            revision: agent.revision,
            head: agent.conversation.last_seq,
            lineage: history_identity(agent),
            through: 0,
            bytes: 0,
            tail_hash: [0; HASH_BYTES],
            state: Preparation::Pending,
            integrity,
        }
    }

    fn read(agent: &Agent, row: &Document) -> Result<Option<Self>, SandboxStoreError> {
        if row.contains_key(QUARANTINE_FIELD) {
            return Err(invalid());
        }
        let Some(value) = row.get(FIELD) else {
            return Ok(None);
        };
        let proof: Self =
            mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| invalid())?;
        if proof.lineage != history_identity(agent)
            || proof.revision > agent.revision
            || proof.head > agent.conversation.last_seq
            || proof.through > proof.head
            || proof.head > PHYSICAL_AGENT_LOG_ENTRIES
            || proof.bytes > PHYSICAL_AGENT_LOG_BYTES
            || (proof.through == 0 && (proof.bytes != 0 || proof.tail_hash != [0; HASH_BYTES]))
            || (proof.state == Preparation::Certified
                && (proof.through == 0 || proof.through != proof.head))
        {
            return Err(invalid());
        }
        Ok((proof.revision == agent.revision
            && proof.head == agent.conversation.last_seq
            && proof.integrity == integrity_tag(row)?)
        .then_some(proof))
    }
}

pub(in crate::store) enum Proof {
    Missing,
    Preparing,
    ByteLimit,
    AggregateBytes,
    Latest(SandboxAgentLogActivity),
}

pub(in crate::store) async fn current(
    store: &MongoSandboxStore,
    agent: &Agent,
    bytes_left: &mut usize,
    session: &mut ClientSession,
) -> Result<Proof, TransactionLogError> {
    let row = legacy_row(store, agent, session).await?;
    let Some(progress) = Progress::read(agent, &row)? else {
        return Ok(Proof::Missing);
    };
    match progress.state {
        Preparation::Pending => return Ok(Proof::Preparing),
        Preparation::ByteLimit => return Ok(Proof::ByteLimit),
        Preparation::Certified => {}
    }
    tip::require_tip(store, agent, session).await?;
    // The structural audit pins the exact content as well as the position.
    // The same revision may still have locally damaged persisted BSON.
    let Some(entry) = anchor(
        store,
        agent,
        progress.through,
        progress.tail_hash,
        bytes_left,
        session,
    )
    .await?
    else {
        return Ok(Proof::AggregateBytes);
    };
    Ok(Proof::Latest(connection_recovery_activity::project_head(
        &entry,
        connection_recovery_activity::activity_time(&row, agent, &entry),
    )))
}

/// Each sweep may commit one page. A new Agent revision or head restarts the
/// audit. The write fence also conflicts with an append during the page.
pub(in crate::store) async fn advance(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Preparation, TransactionLogError> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(TransactionLogError::Driver)?;
    start_transaction(&mut session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let result = advance_in_transaction(store, agent, &mut session).await;
    match result {
        Ok(state) => match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(state),
            _ => Err(SandboxStoreError::Conflict.into()),
        },
        Err(error) => {
            let _ = session.abort_guarded().await;
            Err(error)
        }
    }
}

async fn advance_in_transaction(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Preparation, TransactionLogError> {
    let row = legacy_row(store, agent, session).await?;
    if agent.conversation.last_seq == 0 || agent.conversation.last_seq > PHYSICAL_AGENT_LOG_ENTRIES
    {
        return Err(invalid().into());
    }
    let mut progress = if let Some(progress) = Progress::read(agent, &row)? {
        if progress.state != Preparation::Pending {
            return Ok(progress.state);
        }
        // Each page has a separate allowance for its prior anchor, so two
        // consecutive large rows can each use a full page and make progress.
        let mut anchor_bytes = PAGE_BYTES;
        anchor(
            store,
            agent,
            progress.through,
            progress.tail_hash,
            &mut anchor_bytes,
            session,
        )
        .await?
        .ok_or_else(invalid)?;
        progress
    } else {
        let mut progress = Progress::start(agent, integrity_tag(&row)?);
        if !validate_existing_integrity(store, agent, &row, session).await? {
            progress.state = Preparation::ByteLimit;
            return persist(store, agent, &row, progress, session).await;
        }
        progress
    };
    tip::require_tip(store, agent, session).await?;
    let sizes = page_sizes(
        store,
        agent,
        (progress.through > 0).then_some(progress.through),
        session,
    )
    .await?;
    if sizes.first().is_some_and(|size| *size > PAGE_BYTES) {
        progress.state = Preparation::ByteLimit;
        return persist(store, agent, &row, progress, session).await;
    }
    let mut remaining = PAGE_BYTES;
    let sizes: Vec<_> = sizes
        .into_iter()
        .take_while(|size| {
            if let Some(left) = remaining.checked_sub(*size) {
                remaining = left;
                true
            } else {
                false
            }
        })
        .collect();
    if sizes.is_empty() || progress.through + sizes.len() as u64 > progress.head {
        return Err(invalid().into());
    }
    read_page(store, agent, &mut progress, &sizes, session).await?;
    if progress.through == progress.head {
        progress.state = Preparation::Certified;
    }
    persist(store, agent, &row, progress, session).await
}

async fn validate_existing_integrity(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
) -> Result<bool, TransactionLogError> {
    let Some(integrity) = parse_integrity(row)? else {
        return Ok(true);
    };
    if integrity.head_seq > 0
        && point_size(store, agent, integrity.head_seq, session).await? > PAGE_BYTES
    {
        return Ok(false);
    }
    proof::read_only_summary(store, agent, row, session, false).await?;
    Ok(true)
}

async fn persist(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    progress: Progress,
    session: &mut ClientSession,
) -> Result<Preparation, TransactionLogError> {
    let value = mongodb::bson::serialize_to_document(&progress).map_err(|_| invalid())?;
    let result = store
        .agents
        .update_one(
            envelope_filter(agent, row)?,
            doc! { "$set": { FIELD: value } },
        )
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(progress.state)
}

fn integrity_tag(row: &Document) -> Result<Option<[u8; HASH_BYTES]>, SandboxStoreError> {
    row.get(INTEGRITY_FIELD)
        .map(|value| {
            mongodb::bson::serialize_to_vec(&doc! { "integrity": value.clone() })
                .map(|bytes| hash_raw(&bytes))
                .map_err(|_| invalid())
        })
        .transpose()
}

async fn legacy_row(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Document, TransactionLogError> {
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *agent || read_format(&row)?.is_some() {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(row)
}

// Return only scalar metadata to enforce the aggregate byte budget before
// loading any bodies. Raw BSON below is used solely for the existing hash.
async fn page_sizes(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: Option<u64>,
    session: &mut ClientSession,
) -> Result<Vec<usize>, TransactionLogError> {
    let mut filter = doc! { "agent_id": agent.id.as_str() };
    if let Some(after) = after {
        filter.insert("seq", doc! { "$gt": signed(after, "inactivity sequence").map_err(SandboxStoreError::Corrupt)? });
    }
    let mut cursor = store.agent_log.aggregate([
        doc! { "$match": filter },
        doc! { "$sort": { "seq": 1 } },
        doc! { "$limit": PAGE_ROWS },
        doc! { "$project": { "_id": 0, "seq": 1, "bytes": { "$toLong": { "$bsonSize": "$$ROOT" } } } },
    ]).hint(Hint::Name("agent_log_sequence".into())).session(&mut *session).await
        .map_err(TransactionLogError::Driver)?;
    let mut result = Vec::new();
    let mut expected = after.unwrap_or(0);
    while cursor
        .advance(session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        let row = cursor
            .deserialize_current()
            .map_err(TransactionLogError::Driver)?;
        expected += 1;
        if sequence(&row) != Some(expected) {
            return Err(invalid().into());
        }
        result.push(row_bytes(&row)?);
    }
    Ok(result)
}

async fn anchor(
    store: &MongoSandboxStore,
    agent: &Agent,
    seq: u64,
    expected: [u8; HASH_BYTES],
    bytes_left: &mut usize,
    session: &mut ClientSession,
) -> Result<Option<AgentLogEntry>, TransactionLogError> {
    let size = point_size(store, agent, seq, session).await?;
    let Some(remaining) = bytes_left.checked_sub(size) else {
        return Ok(None);
    };
    let (entry, hash, actual_size) = read_raw(store, agent, seq, session).await?;
    if actual_size != size || hash != expected {
        return Err(invalid().into());
    }
    *bytes_left = remaining;
    Ok(Some(entry))
}

async fn point_size(
    store: &MongoSandboxStore,
    agent: &Agent,
    seq: u64,
    session: &mut ClientSession,
) -> Result<usize, TransactionLogError> {
    let mut cursor = store
        .agent_log
        .aggregate([
            doc! { "$match": { "_id": log_id(&agent.id, seq), "agent_id": agent.id.as_str() } },
            doc! { "$project": { "_id": 0, "bytes": { "$toLong": { "$bsonSize": "$$ROOT" } } } },
        ])
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if !cursor
        .advance(session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        return Err(invalid().into());
    }
    Ok(row_bytes(
        &cursor
            .deserialize_current()
            .map_err(TransactionLogError::Driver)?,
    )?)
}

async fn read_page(
    store: &MongoSandboxStore,
    agent: &Agent,
    progress: &mut Progress,
    sizes: &[usize],
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let first = signed(progress.through + 1, "inactivity page start")
        .map_err(SandboxStoreError::Corrupt)?;
    let last = signed(progress.through + sizes.len() as u64, "inactivity page end")
        .map_err(SandboxStoreError::Corrupt)?;
    let mut cursor = store
        .agent_log
        .clone_with_type::<RawDocumentBuf>()
        .find(doc! { "agent_id": agent.id.as_str(), "seq": { "$gte": first, "$lte": last }})
        .hint(Hint::Name("agent_log_sequence".into()))
        .sort(doc! { "seq": 1 })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    for size in sizes {
        if !cursor
            .advance(session)
            .await
            .map_err(TransactionLogError::Driver)?
        {
            return Err(invalid().into());
        }
        let raw = cursor.current();
        if raw.as_bytes().len() != *size {
            return Err(invalid().into());
        }
        let entry = decode(raw, agent, progress.through + 1)?;
        progress.through = entry.seq;
        progress.tail_hash = hash_raw(raw.as_bytes());
        progress.bytes = progress
            .bytes
            .checked_add(*size)
            .filter(|total| *total <= PHYSICAL_AGENT_LOG_BYTES)
            .ok_or_else(invalid)?;
    }
    Ok(())
}

async fn read_raw(
    store: &MongoSandboxStore,
    agent: &Agent,
    seq: u64,
    session: &mut ClientSession,
) -> Result<(AgentLogEntry, [u8; HASH_BYTES], usize), TransactionLogError> {
    let raw = store
        .agent_log
        .clone_with_type::<RawDocumentBuf>()
        .find_one(doc! { "_id": log_id(&agent.id, seq), "agent_id": agent.id.as_str() })
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(invalid)?;
    let entry = decode(&raw, agent, seq)?;
    Ok((entry, hash_raw(raw.as_bytes()), raw.as_bytes().len()))
}

fn decode(
    raw: &mongodb::bson::RawDocument,
    agent: &Agent,
    seq: u64,
) -> Result<AgentLogEntry, SandboxStoreError> {
    let document = Document::try_from(raw).map_err(|_| invalid())?;
    let entry = read_entry(&document)?;
    if entry.seq != seq
        || entry.agent_id != agent.id
        || matches!(&entry.kind, layer2_core::sandbox::AgentLogKind::Created { workspace_id, worktree_id }
            if seq != 1 || workspace_id != &agent.workspace_id || worktree_id != agent.creation_worktree_id())
    {
        return Err(invalid());
    }
    Ok(entry)
}

fn sequence(row: &Document) -> Option<u64> {
    match row.get("seq")? {
        Bson::Int32(value) => u64::try_from(*value).ok(),
        Bson::Int64(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn row_bytes(row: &Document) -> Result<usize, SandboxStoreError> {
    row.get_i64("bytes")
        .ok()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|size| *size > 0 && *size <= MAX_AGENT_LOG_DOCUMENT_BYTES)
        .ok_or_else(invalid)
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::Corrupt("invalid legacy Agent inactivity proof".into())
}
