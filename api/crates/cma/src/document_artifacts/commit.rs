//! Metadata writes that atomically publish a reserved source object.
use super::mongo::{encode, MongoDocumentStore};
use super::quota::object_row;
use crate::store::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
};
use layer2_core::document_artifacts::{DocumentError, DocumentSourceReservation, StoredDocument};
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
};
use std::time::Duration;

const TRANSACTION_ATTEMPTS: usize = 8;
const IO: Duration = Duration::from_secs(3);

pub(super) async fn insert(
    store: &MongoDocumentStore,
    record: &StoredDocument,
    source: &DocumentSourceReservation,
) -> Result<bool, DocumentError> {
    write(store, record, None, source).await
}

pub(super) async fn compare_exchange(
    store: &MongoDocumentStore,
    expected: u64,
    record: &StoredDocument,
    source: &DocumentSourceReservation,
) -> Result<bool, DocumentError> {
    write(store, record, Some(expected), source).await
}

async fn write(
    store: &MongoDocumentStore,
    record: &StoredDocument,
    expected: Option<u64>,
    source: &DocumentSourceReservation,
) -> Result<bool, DocumentError> {
    let encoded = encode(record)?;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .rows
            .client()
            .start_session()
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) && attempt + 1 < TRANSACTION_ATTEMPTS {
                continue;
            }
            return Err(DocumentError::Unavailable);
        }
        let applied = if let Some(expected) = expected {
            let expected = i64::try_from(expected).map_err(|_| DocumentError::Corrupt)?;
            if record.metadata.revision != expected as u64 + 1 {
                let _ = session.abort_transaction().await;
                return Err(DocumentError::Corrupt);
            }
            store
                .rows
                .replace_one(
                    doc! {
                        "_id": &record.metadata.id,
                        "owner_subject": &record.metadata.owner_subject,
                        "revision": expected
                    },
                    encoded.clone(),
                )
                .session(&mut session)
                .await
                .map(|result| result.matched_count == 1)
        } else {
            store
                .rows
                .insert_one(encoded.clone())
                .session(&mut session)
                .await
                .map(|_| true)
        };
        match applied {
            Ok(true) => {}
            Ok(false) => {
                let _ = session.abort_transaction().await;
                return Ok(false);
            }
            Err(error) if is_duplicate_key(&error) => {
                let _ = session.abort_transaction().await;
                return Ok(false);
            }
            Err(error) if is_transient(&error) && attempt + 1 < TRANSACTION_ATTEMPTS => {
                let _ = session.abort_transaction().await;
                tokio::time::sleep(Duration::from_millis(((attempt + 1).min(16)) as u64)).await;
                continue;
            }
            Err(_) => {
                let _ = session.abort_transaction().await;
                return Err(DocumentError::Unavailable);
            }
        };
        let object = object_row(source)?;
        let object_id = object.get_str("_id").map_err(|_| DocumentError::Corrupt)?;
        let marked = store
            .quota_rows()
            .update_one(
                doc! {
                    "_id": object_id,
                    "$or": [
                        {"state": {"$in": ["reserved", "committed"]}},
                        {"state": {"$exists": false}}
                    ]
                },
                doc! {"$set": {"state": "committed"}},
            )
            .session(&mut session)
            .await;
        match marked {
            Ok(result) if result.matched_count == 1 => {}
            Ok(_) => {
                let _ = session.abort_transaction().await;
                return Err(DocumentError::Unavailable);
            }
            Err(error) if is_transient(&error) && attempt + 1 < TRANSACTION_ATTEMPTS => {
                let _ = session.abort_transaction().await;
                continue;
            }
            Err(_) => {
                let _ = session.abort_transaction().await;
                return Err(DocumentError::Unavailable);
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(true),
            CommitOutcome::Retry(error)
                if is_transient(&error) && attempt + 1 < TRANSACTION_ATTEMPTS =>
            {
                continue;
            }
            CommitOutcome::Unknown(_) => {
                return reconcile(store, &encoded, object_id, session.operation_time()).await;
            }
            CommitOutcome::Retry(_) | CommitOutcome::Failed(_) => {
                return Err(DocumentError::Unavailable);
            }
        }
    }
    Err(DocumentError::Unavailable)
}

async fn reconcile(
    store: &MongoDocumentStore,
    expected: &Document,
    object_id: &str,
    operation_time: Option<Timestamp>,
) -> Result<bool, DocumentError> {
    let time = operation_time.ok_or(DocumentError::Unavailable)?;
    let mut snapshot = store
        .rows
        .client()
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|_| DocumentError::Unavailable)?;
    let metadata = store
        .rows
        .find_one(doc! {"_id": expected.get("_id").cloned().ok_or(DocumentError::Corrupt)?})
        .read_concern(ReadConcern::snapshot())
        .max_time(IO)
        .session(&mut snapshot)
        .await
        .map_err(|_| DocumentError::Unavailable)?;
    let object = store
        .quota_rows()
        .find_one(doc! {"_id": object_id})
        .read_concern(ReadConcern::snapshot())
        .max_time(IO)
        .session(&mut snapshot)
        .await
        .map_err(|_| DocumentError::Unavailable)?;
    if snapshot_matches(metadata.as_ref(), object.as_ref(), expected) {
        Ok(true)
    } else {
        Err(DocumentError::Unavailable)
    }
}

pub(super) fn snapshot_matches(
    metadata: Option<&Document>,
    object: Option<&Document>,
    expected: &Document,
) -> bool {
    metadata == Some(expected)
        && object.is_some_and(|row| row.get_str("state").ok() == Some("committed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_commit_unknown_result_is_resolved_by_snapshot_read() {
        let expected = doc! {"_id": "doc_1", "record": {"revision": 2}};
        let committed = doc! {"_id": "object", "state": "committed"};
        assert!(snapshot_matches(
            Some(&expected),
            Some(&committed),
            &expected
        ));
        let reserved = doc! {"_id": "object", "state": "reserved"};
        assert!(!snapshot_matches(
            Some(&expected),
            Some(&reserved),
            &expected
        ));
        assert!(!snapshot_matches(None, Some(&committed), &expected));
    }
}
