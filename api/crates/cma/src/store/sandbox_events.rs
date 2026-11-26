//! Gap-free Mongo event stream for sandbox lifecycle changes.

mod retry;

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind};
use mongodb::bson::{doc, Document};
use mongodb::options::{IndexOptions, ReadConcern};
use mongodb::{ClientSession, Collection, IndexModel};

use super::sandbox_mongo::{
    commit_transaction, i64_field_matches, is_duplicate_key, is_transient, read_record,
    record_bson, signed, start_transaction, string_field_matches, unsigned,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};

pub(super) fn sandbox_event_indexes() -> Vec<IndexModel> {
    let sequence = IndexModel::builder()
        .keys(doc! { "sandbox_id": 1, "seq": 1 })
        .options(
            IndexOptions::builder()
                .name("sandbox_events_sequence".to_string())
                .unique(true)
                .build(),
        )
        .build();
    std::iter::once(sequence)
        .chain(super::retained_reopen::receipt_indexes())
        .collect()
}

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(sandbox_event_indexes())
        .await
        .map(|_| ())
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("sandbox event indexes: {error}"))
        })
}

pub(super) fn event_id(id: &SandboxId, seq: u64) -> String {
    format!("{}:{seq:020}", id.as_str())
}

pub(super) fn event_document(event: &SandboxEvent) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": event_id(&event.sandbox_id, event.seq),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": event.sandbox_id.as_str(),
        "seq": signed(event.seq, "sandbox event sequence")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(event).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_event(document: &Document) -> Result<SandboxEvent, SandboxStoreError> {
    let event: SandboxEvent = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let seq = signed(event.seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", &event_id(&event.sandbox_id, event.seq))
        || !string_field_matches(document, "sandbox_id", event.sandbox_id.as_str())
        || !i64_field_matches(document, "seq", seq)
    {
        return Err(SandboxStoreError::Corrupt(
            "sandbox event query fields disagree with its record".into(),
        ));
    }
    Ok(event)
}

pub(super) enum TransactionEventError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn validate_in_transaction(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    last_seq: u64,
    session: &mut ClientSession,
) -> Result<(), TransactionEventError> {
    let mut read = super::mongo_read_telemetry::Read::find(store.sandbox_events.name());
    let mut cursor = store
        .sandbox_events
        .find(doc! { "sandbox_id": sandbox_id.as_str() })
        .sort(doc! { "seq": 1 })
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?;
    let mut sequence = 0_u64;
    while let Some(document) = read
        .next_in_session(&mut cursor, &mut *session)
        .await
        .map_err(TransactionEventError::Driver)?
    {
        let event = read_event(&document).map_err(TransactionEventError::Domain)?;
        if sequence.checked_add(1) != Some(event.seq) || event.seq > last_seq {
            return Err(TransactionEventError::Domain(
                SandboxStoreError::SequenceConflict,
            ));
        }
        sequence = event.seq;
    }
    if sequence != last_seq {
        return Err(TransactionEventError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    read.complete();
    Ok(())
}

pub(super) async fn append_in_transaction(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    sandbox_id: &SandboxId,
    appended_at: String,
    kind: SandboxEventKind,
) -> Result<SandboxEvent, TransactionEventError> {
    append_with_receipt(store, session, sandbox_id, appended_at, kind, None).await
}

pub(super) async fn append_with_receipt(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    sandbox_id: &SandboxId,
    appended_at: String,
    kind: SandboxEventKind,
    receipt: Option<Document>,
) -> Result<SandboxEvent, TransactionEventError> {
    let aggregate = store
        .sandboxes
        .find_one(crate::store::work_schema::query(doc! {
            "_id": sandbox_id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        }))
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?
        .ok_or(TransactionEventError::Domain(SandboxStoreError::NotFound))?;
    crate::store::work_schema::require(&aggregate)
        .map_err(SandboxStoreError::Corrupt)
        .map_err(TransactionEventError::Domain)?;
    let previous = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| unsigned(seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt))
        .map_err(TransactionEventError::Domain)?;
    let seq = previous
        .checked_add(1)
        .ok_or(TransactionEventError::Domain(
            SandboxStoreError::SequenceConflict,
        ))?;
    let advanced = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(doc! {
                "_id": sandbox_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "last_seq": signed(previous, "sandbox event sequence")
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(TransactionEventError::Domain)?,
            }),
            doc! { "$set": { "last_seq": signed(seq, "sandbox event sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(TransactionEventError::Domain)? } },
        )
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?;
    if advanced.matched_count != 1 {
        return Err(TransactionEventError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    let event = SandboxEvent {
        sandbox_id: sandbox_id.clone(),
        seq,
        appended_at,
        kind,
    };
    let mut document = event_document(&event).map_err(TransactionEventError::Domain)?;
    if let Some(receipt) = receipt {
        document.insert("retained_reopen_receipt", receipt);
    }
    store
        .sandbox_events
        .insert_one(document)
        .session(&mut *session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                TransactionEventError::Domain(SandboxStoreError::SequenceConflict)
            } else {
                TransactionEventError::Driver(error)
            }
        })?;
    super::sandbox_usage::lifecycle_event(store, session, &event, None).await?;
    Ok(event)
}

pub(super) async fn reconcile_event(
    store: &MongoSandboxStore,
    expected: &SandboxEvent,
) -> Result<Option<SandboxEvent>, SandboxStoreError> {
    let found = store
        .sandbox_events
        .find_one(doc! { "_id": event_id(&expected.sandbox_id, expected.seq) })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    match found {
        Some(document) => {
            let actual = read_event(&document)?;
            if &actual == expected {
                Ok(Some(actual))
            } else {
                Err(SandboxStoreError::SequenceConflict)
            }
        }
        None => Ok(None),
    }
}

pub(super) async fn reconcile_event_with_snapshot_session(
    store: &MongoSandboxStore,
    expected: &SandboxEvent,
    session: &mut ClientSession,
) -> Result<Option<SandboxEvent>, SandboxStoreError> {
    let found = store
        .sandbox_events
        .find_one(doc! { "_id": event_id(&expected.sandbox_id, expected.seq) })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    match found {
        Some(document) => {
            let actual = read_event(&document)?;
            if &actual == expected {
                Ok(Some(actual))
            } else {
                Err(SandboxStoreError::SequenceConflict)
            }
        }
        None => Ok(None),
    }
}

async fn reconcile_append_after_unknown(
    store: &MongoSandboxStore,
    expected: &SandboxEvent,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<Option<SandboxEvent>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let aggregate = store
        .sandboxes
        .find_one(doc! { "_id": expected.sandbox_id.as_str() })
        .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(aggregate) = aggregate else {
        return Ok(None);
    };
    crate::store::work_schema::require(&aggregate).map_err(SandboxStoreError::Corrupt)?;
    let last_seq = aggregate
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| {
            unsigned(seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt)
        })?;
    if last_seq != expected.seq {
        return Ok(None);
    }
    reconcile_event_with_snapshot_session(store, expected, &mut session).await
}

#[async_trait::async_trait]
impl SandboxEventStore for MongoSandboxStore {
    async fn lifecycle_retry_started_at(
        &self,
        sandbox_id: &SandboxId,
        admitted_at: &str,
    ) -> Result<Option<String>, SandboxStoreError> {
        retry::started_at(self, sandbox_id, admitted_at).await
    }

    async fn append_sandbox_event(
        &self,
        sandbox_id: &SandboxId,
        appended_at: String,
        kind: SandboxEventKind,
    ) -> Result<SandboxEvent, SandboxStoreError> {
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
            let event = match append_in_transaction(
                self,
                &mut session,
                sandbox_id,
                appended_at.clone(),
                kind.clone(),
            )
            .await
            {
                Ok(event) => event,
                Err(TransactionEventError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(TransactionEventError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(TransactionEventError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(event),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let snapshot_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "sandbox event commit remained unknown without an operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved_error = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("sandbox event commit", async move {
                        reconcile_append_after_unknown(&store, &event, snapshot_time)
                            .await?
                            .ok_or_else(|| {
                                SandboxStoreError::Unavailable(format!(
                                    "sandbox event commit remained unknown: {unresolved_error}"
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
            "sandbox event transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn sandbox_events(
        &self,
        sandbox_id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let Some(aggregate) = self
            .sandboxes
            .find_one(doc! { "_id": sandbox_id.as_str() })
            .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        else {
            return Ok(Vec::new());
        };
        crate::store::work_schema::require(&aggregate).map_err(SandboxStoreError::Corrupt)?;
        let last_seq = aggregate
            .get_i64("last_seq")
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            .and_then(|value| {
                unsigned(value, "sandbox event sequence").map_err(SandboxStoreError::Corrupt)
            })?;
        if after >= last_seq {
            return Ok(Vec::new());
        }
        let after_signed =
            signed(after, "sandbox event cursor").map_err(|_| SandboxStoreError::InvalidPage)?;
        let high_water =
            signed(last_seq, "sandbox event high water").map_err(SandboxStoreError::Corrupt)?;
        let mut read = super::mongo_read_telemetry::Read::find(self.sandbox_events.name());
        let mut cursor = self
            .sandbox_events
            .find(doc! {
                "sandbox_id": sandbox_id.as_str(),
                "seq": { "$gt": after_signed, "$lte": high_water },
            })
            .sort(doc! { "seq": 1 })
            .limit(i64::from(limit))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut events = Vec::new();
        while let Some(document) = read
            .next(&mut cursor)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            events.push(read_event(&document)?);
        }
        verify_contiguous(after, last_seq, limit, &events)?;
        read.complete();
        Ok(events)
    }
}

pub(super) fn verify_contiguous(
    after: u64,
    last_seq: u64,
    limit: u16,
    events: &[SandboxEvent],
) -> Result<(), SandboxStoreError> {
    let available = last_seq.saturating_sub(after);
    let expected_len = available.min(u64::from(limit)) as usize;
    if events.len() != expected_len {
        return Err(SandboxStoreError::SequenceConflict);
    }
    for (offset, event) in events.iter().enumerate() {
        let expected = after
            .checked_add(offset as u64 + 1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if event.seq != expected {
            return Err(SandboxStoreError::SequenceConflict);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_index_and_gap_detector_are_exact() {
        let index = sandbox_event_indexes().remove(0);
        assert_eq!(index.keys, doc! { "sandbox_id": 1, "seq": 1 });
        assert_eq!(index.options.unwrap().unique, Some(true));
        let id = SandboxId("sbx_test".into());
        let event = |seq| SandboxEvent {
            sandbox_id: id.clone(),
            seq,
            appended_at: "2026-09-05T00:00:00Z".into(),
            kind: SandboxEventKind::Created,
        };
        assert!(verify_contiguous(0, 2, 2, &[event(1), event(2)]).is_ok());
        assert_eq!(
            verify_contiguous(0, 2, 2, &[event(2)]),
            Err(SandboxStoreError::SequenceConflict)
        );
        assert_eq!(
            verify_contiguous(0, 1, 2, &[]),
            Err(SandboxStoreError::SequenceConflict)
        );
    }
}
