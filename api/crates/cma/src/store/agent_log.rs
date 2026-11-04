//! Gap-free Mongo log for agent conversation and tool events.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReadConcern};
use mongodb::{Collection, IndexModel};

use super::agent_log_integrity::{
    ensure_append_capacity, scan_complete_log, AppendCapacity, LogSummary, TransactionLogError,
};
use super::agents::read_agent;
use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, read_record,
    record_bson, require_version, signed, start_transaction, string_field_matches, unsigned,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
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

async fn scan_read_log<F>(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    visit: F,
) -> Result<(), SandboxStoreError>
where
    F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
{
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(aggregate) = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(());
    };
    require_version(&aggregate).map_err(SandboxStoreError::Corrupt)?;
    let last_seq = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|value| {
            unsigned(value, "agent log sequence").map_err(SandboxStoreError::Corrupt)
        })?;
    scan_complete_log_with_snapshot_session(store, agent_id, last_seq, &mut session, visit)
        .await
        .map_err(transaction_error)?;
    Ok(())
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

async fn append_in_transaction(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    agent_id: &AgentId,
    appended_at: String,
    kind: AgentLogKind,
) -> Result<AgentLogEntry, TransactionLogError> {
    let aggregate = store
        .agents
        .find_one(doc! {
            "_id": agent_id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        })
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
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
    if matches!(entry.kind, AgentLogKind::TurnCompleted { .. }) {
        let row = store
            .agents
            .find_one(doc! {"_id":agent_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(TransactionLogError::Domain(SandboxStoreError::NotFound))?;
        let agent = read_agent(&row).map_err(TransactionLogError::Domain)?;
        super::suggestions::enqueue(store, &agent, std::slice::from_ref(&entry), session).await?;
    }
    Ok(entry)
}

async fn reconcile_append_after_unknown(
    store: &MongoSandboxStore,
    expected: &AgentLogEntry,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let agent = store
        .agents
        .find_one(doc! { "_id": expected.agent_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
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
    let mut found = None;
    scan_complete_log_with_snapshot_session(
        store,
        &expected.agent_id,
        agent.conversation.last_seq,
        &mut session,
        |entry| {
            if entry.seq == expected.seq {
                if &entry != expected {
                    return Err(SandboxStoreError::SequenceConflict);
                }
                found = Some(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    })?;
    Ok(found)
}

#[async_trait::async_trait]
impl AgentLogStore for MongoSandboxStore {
    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
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
            let entry = match append_in_transaction(
                self,
                &mut session,
                agent_id,
                appended_at.clone(),
                kind.clone(),
            )
            .await
            {
                Ok(entry) => entry,
                Err(TransactionLogError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(TransactionLogError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(TransactionLogError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(entry),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let snapshot_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent log commit remained unknown without an operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved_error = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("agent log commit", async move {
                        reconcile_append_after_unknown(&store, &entry, snapshot_time)
                            .await?
                            .ok_or_else(|| {
                                SandboxStoreError::Unavailable(format!(
                                    "agent log commit remained unknown: {unresolved_error}"
                                ))
                            })
                    })
                    .await
                    .map_err(|deadline| {
                        SandboxStoreError::Unavailable(format!(
                            "{deadline}; ambiguous commit: {commit_error}"
                        ))
                    })?;
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "agent log transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut page = Vec::with_capacity(usize::from(limit));
        scan_read_log(self, agent_id, |entry| {
            if entry.seq > after && page.len() < usize::from(limit) {
                page.push(entry);
            }
            Ok(())
        })
        .await?;
        Ok(page)
    }

    async fn agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        let expected = (through - after).min(u64::from(limit)) as usize;
        if expected == 0 {
            // Empty pages still validate the captured snapshot so a cursor at
            // the high-water mark cannot hide a gap elsewhere in the log.
            scan_read_log(self, agent_id, |_| Ok(())).await?;
            return Ok(Vec::new());
        }
        let mut page = Vec::with_capacity(expected);
        scan_read_log(self, agent_id, |entry| {
            if entry.seq > after && entry.seq <= through && page.len() < expected {
                page.push(entry);
            }
            Ok(())
        })
        .await?;
        if page.len() != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok(page)
    }

    async fn agent_log_entry(
        &self,
        agent_id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        if sequence == 0 {
            return Ok(None);
        }
        self.agent_log
            .find_one(doc! {"_id": log_id(agent_id, sequence)})
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_entry)
            .transpose()
    }

    async fn agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let mut entries = Vec::new();
        // Validate the entire current snapshot once, including records beyond the prefix.
        scan_read_log(self, agent_id, |entry| {
            if entry.seq <= through_seq {
                entries.push(entry);
            }
            Ok(())
        })
        .await?;
        if u64::try_from(entries.len()).ok() != Some(through_seq) {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok(entries)
    }
}

fn transaction_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
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
