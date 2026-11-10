//! Gap-free Mongo log for agent conversation and tool events.

use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReadConcern};
use mongodb::{Collection, IndexModel};

use super::agent_log_integrity::{
    prepare_append, trusted_summary, AppendCapacity, SemanticCheckpoint, TransactionLogError,
};
pub(super) use super::agent_log_integrity::{
    scan_complete_log_with_session, scan_complete_log_with_snapshot_session,
};
use super::agent_log_reads::{read_window, Window};
use super::agents::read_agent;
use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, read_record,
    record_bson, require_version, signed, start_transaction, string_field_matches,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};

mod receipt;
use receipt::reconcile_append_after_unknown;
mod latest_turn;
#[cfg(test)]
mod receipt_tests;

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

pub(super) fn log_id(id: &AgentId, seq: u64) -> String {
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

pub(in crate::store) async fn append_in_transaction(
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
    let agent = read_agent(&aggregate).map_err(TransactionLogError::Domain)?;
    let previous = agent.conversation.last_seq;
    let (complete, semantic) = match trusted_summary(
        store,
        &agent,
        &aggregate,
        ScanCaller::Other,
        session,
        false,
    )
    .await?
    {
        Some(summary) => {
            let semantic = summary.semantic().cloned().ok_or_else(|| {
                TransactionLogError::Domain(SandboxStoreError::Corrupt(
                    "trusted Agent log metadata has no semantic checkpoint".into(),
                ))
            })?;
            (summary, semantic)
        }
        None => {
            let mut entries = Vec::new();
            let summary = scan_complete_log_with_session(
                ScanCaller::Other,
                store,
                agent_id,
                previous,
                session,
                |entry| {
                    entries.push(entry);
                    Ok(())
                },
            )
            .await?;
            let semantic = crate::store::agent_log_integrity::audit_checkpoint(
                store, &agent, &entries, session,
            )
            .await?;
            (summary, semantic)
        }
    };
    let seq = previous.checked_add(1).ok_or(TransactionLogError::Domain(
        SandboxStoreError::SequenceConflict,
    ))?;
    let entry = AgentLogEntry {
        agent_id: agent_id.clone(),
        seq,
        appended_at,
        kind,
    };

    let mut next_agent = agent.clone();
    next_agent.conversation.last_seq = seq;
    let semantic = semantic
        .after_append(std::slice::from_ref(&entry), &next_agent)
        .map_err(TransactionLogError::Domain)?;
    let integrity = prepare_append(
        &agent,
        &complete,
        std::slice::from_ref(&entry),
        agent.revision,
        semantic,
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
                    "agent_log_integrity_v1": integrity.integrity,
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
        .clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .insert_one(&integrity.documents[0])
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

#[async_trait::async_trait]
impl AgentLogStore for MongoSandboxStore {
    async fn agent_latest_started_turn(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<Option<u64>, SandboxStoreError> {
        latest_turn::read(self, agent).await
    }

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
        read_window(self, agent_id, Window::Page { after, limit }).await
    }

    async fn agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        read_window(
            self,
            agent_id,
            Window::Range {
                after,
                through,
                limit,
            },
        )
        .await
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
        read_window(
            self,
            agent_id,
            Window::Prefix {
                through: through_seq,
            },
        )
        .await
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
