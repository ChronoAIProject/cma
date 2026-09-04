//! Gap-free Mongo log for agent conversation and tool events.

use futures_util::TryStreamExt;
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, read_record,
    record_bson, require_version, signed, start_transaction, string_field_matches, unsigned,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
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

fn log_document(entry: &AgentLogEntry) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": log_id(&entry.agent_id, entry.seq),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "agent_id": entry.agent_id.as_str(),
        "seq": signed(entry.seq, "agent log sequence").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(entry).map_err(SandboxStoreError::Corrupt)?,
    })
}

fn read_entry(document: &Document) -> Result<AgentLogEntry, SandboxStoreError> {
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

enum TransactionLogError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
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
    let seq = previous.checked_add(1).ok_or(TransactionLogError::Domain(
        SandboxStoreError::SequenceConflict,
    ))?;
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
    let entry = AgentLogEntry {
        agent_id: agent_id.clone(),
        seq,
        appended_at,
        kind,
    };
    store
        .agent_log
        .insert_one(log_document(&entry).map_err(TransactionLogError::Domain)?)
        .session(session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                TransactionLogError::Domain(SandboxStoreError::SequenceConflict)
            } else {
                TransactionLogError::Driver(error)
            }
        })?;
    Ok(entry)
}

async fn reconcile_entry(
    store: &MongoSandboxStore,
    expected: &AgentLogEntry,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let found = store
        .agent_log
        .find_one(doc! { "_id": log_id(&expected.agent_id, expected.seq) })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    match found {
        Some(document) => {
            let actual = read_entry(&document)?;
            if &actual == expected {
                Ok(Some(actual))
            } else {
                Err(SandboxStoreError::SequenceConflict)
            }
        }
        None => Ok(None),
    }
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
                    return reconcile_entry(self, &entry).await?.ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent log commit remained unknown: {error}"
                        ))
                    });
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
        let Some(aggregate) = self
            .agents
            .find_one(doc! { "_id": agent_id.as_str() })
            .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        else {
            return Ok(Vec::new());
        };
        require_version(&aggregate).map_err(SandboxStoreError::Corrupt)?;
        let last_seq = aggregate
            .get_i64("last_seq")
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            .and_then(|value| {
                unsigned(value, "agent log sequence").map_err(SandboxStoreError::Corrupt)
            })?;
        if after >= last_seq {
            return Ok(Vec::new());
        }
        let after_signed =
            signed(after, "agent log cursor").map_err(|_| SandboxStoreError::InvalidPage)?;
        let high_water =
            signed(last_seq, "agent log high water").map_err(SandboxStoreError::Corrupt)?;
        let mut cursor = self
            .agent_log
            .find(doc! {
                "agent_id": agent_id.as_str(),
                "seq": { "$gt": after_signed, "$lte": high_water },
            })
            .sort(doc! { "seq": 1 })
            .limit(i64::from(limit))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut entries = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            entries.push(read_entry(&document)?);
        }
        verify_contiguous(after, last_seq, limit, &entries)?;
        Ok(entries)
    }
}

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
mod tests {
    use super::*;

    #[test]
    fn agent_log_index_is_unique_and_gap_detection_fails_closed() {
        let index = agent_log_indexes().remove(0);
        assert_eq!(index.keys, doc! { "agent_id": 1, "seq": 1 });
        assert_eq!(index.options.unwrap().unique, Some(true));
        let id = AgentId("agt_test".into());
        let entry = |seq| AgentLogEntry {
            agent_id: id.clone(),
            seq,
            appended_at: "2026-09-05T00:00:00Z".into(),
            kind: AgentLogKind::Stopped {
                reason: "done".into(),
            },
        };
        assert!(verify_contiguous(1, 3, 2, &[entry(2), entry(3)]).is_ok());
        assert_eq!(
            verify_contiguous(1, 3, 2, &[entry(3)]),
            Err(SandboxStoreError::SequenceConflict)
        );
    }
}
