//! Gap-free Mongo log for agent conversation and tool events.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReadConcern};
use mongodb::{Collection, IndexModel};

use super::agent_log_integrity::{
    ensure_append_capacity, scan_complete_log, AppendCapacity, LogSummary, TransactionLogError,
};
use super::agents::read_agent;
use super::sandbox_mongo::{
    i64_field_matches, is_duplicate_key, read_record, record_bson, require_version, signed,
    string_field_matches, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) fn agent_log_indexes() -> Vec<IndexModel> {
    vec![IndexModel::builder()
        .keys(doc! { "agent_id": 1, "seq": 1 })
        .options(
            IndexOptions::builder()
                .name("agent_log_sequence".to_string())
                .unique(true)
                .build(),
        )
        .build()]
}

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(agent_log_indexes())
        .await
        .map(|_| ())
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("agent log indexes: {error}"))
        })
}

fn log_id(id: &AgentId, seq: u64) -> String {
    format!("{}:{seq:020}", id.as_str())
}

pub(super) fn log_document(entry: &AgentLogEntry) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": log_id(&entry.agent_id, entry.seq),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "agent_id": entry.agent_id.as_str(),
        "seq": signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(entry).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_entry(document: &Document) -> Result<AgentLogEntry, SandboxStoreError> {
    let entry: AgentLogEntry = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let seq = signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", &log_id(&entry.agent_id, entry.seq))
        || !string_field_matches(document, "agent_id", entry.agent_id.as_str())
        || !i64_field_matches(document, "seq", seq)
    {
        return Err(SandboxStoreError::Corrupt(
            "agent log query fields disagree with its record".into(),
        ));
    }
    Ok(entry)
}

pub(super) async fn scan_complete_log_with_session<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_complete_log(store, agent_id, last_seq, session, false, visit).await
}

pub(super) async fn scan_complete_log_with_snapshot_session<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    last_seq: u64,
    session: &mut mongodb::ClientSession,
    visit: F,
) -> Result<LogSummary, TransactionLogError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    scan_complete_log(store, agent_id, last_seq, session, true, visit).await
}

#[cfg(test)]
pub(super) fn verify_complete_log(
    agent_id: &AgentId,
    last_seq: u64,
    logs: &[AgentLogEntry],
) -> Result<(), SandboxStoreError> {
    let expected_len =
        usize::try_from(last_seq).map_err(|_| SandboxStoreError::SequenceConflict)?;
    if logs.len() != expected_len
        || logs
            .iter()
            .enumerate()
            .any(|(offset, entry)| entry.agent_id != *agent_id || entry.seq != offset as u64 + 1)
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(())
}

pub(super) struct AppendReceipt {
    pub(super) agent: Agent,
    pub(super) entry: AgentLogEntry,
    pub(super) publication: Option<super::agent_log_segments::publication::PublicationIdentity>,
}

pub(super) async fn append_in_transaction(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    agent_id: &AgentId,
    appended_at: String,
    kind: AgentLogKind,
) -> Result<AppendReceipt, TransactionLogError> {
    let aggregate = store
        .agents
        .find_one(doc! {
            "_id": agent_id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let Some(aggregate) = aggregate else {
        let exists = store
            .agents
            .find_one(doc! { "_id": agent_id.as_str() })
            .projection(doc! { "_id": 1 })
            .session(session)
            .await
            .map_err(TransactionLogError::Driver)?
            .is_some();
        return Err(TransactionLogError::Domain(if exists {
            SandboxStoreError::SequenceConflict
        } else {
            SandboxStoreError::NotFound
        }));
    };
    require_version(&aggregate)
        .map_err(SandboxStoreError::Corrupt)
        .map_err(TransactionLogError::Domain)?;
    let mut published_agent = read_agent(&aggregate).map_err(TransactionLogError::Domain)?;
    let previous = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| unsigned(seq, "agent log sequence").map_err(SandboxStoreError::Corrupt))
        .map_err(TransactionLogError::Domain)?;
    let complete =
        scan_complete_log_with_session(store, agent_id, previous, session, |_| Ok(())).await?;
    let seq = previous.checked_add(1).ok_or(TransactionLogError::Domain(
        SandboxStoreError::SequenceConflict,
    ))?;
    let entry = AgentLogEntry {
        agent_id: agent_id.clone(),
        seq,
        appended_at,
        kind,
    };
    ensure_append_capacity(
        &complete,
        std::slice::from_ref(&entry),
        AppendCapacity::Normal,
    )
    .map_err(TransactionLogError::Domain)?;
    let previous = signed(previous, "agent log sequence")
        .map_err(SandboxStoreError::Corrupt)
        .map_err(TransactionLogError::Domain)?;
    let next = signed(seq, "agent log sequence")
        .map_err(SandboxStoreError::Corrupt)
        .map_err(TransactionLogError::Domain)?;
    let advanced = store
        .agents
        .update_one(
            doc! {
                "_id": agent_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "last_seq": previous,
                "record.conversation.last_seq": previous,
                "channel_history_seal": { "$exists": false },
            },
            doc! {
                "$set": {
                    "last_seq": next,
                    "record.conversation.last_seq": next,
                }
            },
        )
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if advanced.matched_count != 1 {
        return Err(TransactionLogError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    store
        .agent_log
        .insert_one(log_document(&entry).map_err(TransactionLogError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                TransactionLogError::Domain(SandboxStoreError::SequenceConflict)
            } else {
                TransactionLogError::Driver(error)
            }
        })?;
    published_agent.conversation.last_seq = seq;
    if matches!(entry.kind, AgentLogKind::TurnCompleted { .. }) {
        super::suggestions::enqueue(
            store,
            &published_agent,
            std::slice::from_ref(&entry),
            session,
        )
        .await?;
    }
    Ok(AppendReceipt {
        agent: published_agent,
        entry,
        publication: None,
    })
}

pub(super) async fn reconcile_append_after_unknown(
    store: &MongoSandboxStore,
    receipt: &AppendReceipt,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    reconcile_append_snapshot(store, receipt, &mut session).await
}

pub(super) async fn reconcile_append_snapshot(
    store: &MongoSandboxStore,
    receipt: &AppendReceipt,
    session: &mut mongodb::ClientSession,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let expected = &receipt.entry;
    let digest =
        super::agent_log_segments::publication::result_digest(&(&receipt.agent, expected))?;
    if let Some(committed) = super::agent_log_segments::publication::reconcile(
        store,
        receipt.publication.as_ref(),
        &receipt.agent,
        std::slice::from_ref(expected),
        &digest,
        session,
    )
    .await?
    {
        return Ok(committed.then(|| expected.clone()));
    }
    let agent = store
        .agents
        .find_one(doc! { "_id": expected.agent_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?;
    let Some(agent) = agent else {
        return Ok(None);
    };
    if agent.conversation.last_seq != expected.seq {
        return Ok(None);
    }
    Ok(
        super::agent_log_receipts::matches(store, &agent, std::slice::from_ref(expected), session)
            .await?
            .then(|| expected.clone()),
    )
}

#[cfg(test)]
fn verify_contiguous(
    after: u64,
    last_seq: u64,
    limit: u16,
    entries: &[AgentLogEntry],
) -> Result<(), SandboxStoreError> {
    let expected_len = last_seq.saturating_sub(after).min(u64::from(limit)) as usize;
    if entries.len() != expected_len {
        return Err(SandboxStoreError::SequenceConflict);
    }
    for (offset, entry) in entries.iter().enumerate() {
        if entry.seq != after + offset as u64 + 1 {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_log_tests.rs"]
mod tests;
