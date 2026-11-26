//! Commit Document metadata and its immutable source marker in one transaction.
use super::{
    mongo::{decode, encode, MongoDocumentStore},
    quota::{object_row, IO},
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    commit_transaction, is_confirmed_body_retry, is_duplicate_key, start_transaction, CommitOutcome,
};
use layer2_core::document_artifacts::{
    digest, DocumentError, DocumentSourceReservation, StoredDocument,
};
use mongodb::{
    bson::{doc, Document, Timestamp},
    error::UNKNOWN_TRANSACTION_COMMIT_RESULT,
    options::ReadConcern,
    ClientSession, Collection,
};

const ATTEMPTS: usize = 5;

struct Publication<'a> {
    record: &'a StoredDocument,
    metadata: Document,
    source: &'a DocumentSourceReservation,
    reservation: Document,
    marker: Document,
    edit: Option<(i64, &'a str, &'a str)>,
}

pub(super) async fn publish(
    store: &MongoDocumentStore,
    record: &StoredDocument,
    source: &DocumentSourceReservation,
    edit: Option<(u64, &str)>,
) -> Result<bool, DocumentError> {
    let publication = prepare(record, source, edit)?;
    tokio::time::timeout(IO, publication.run(store))
        .await
        .map_err(|_| {
            tracing::warn!("document source publication reached its transaction deadline");
            DocumentError::Unavailable
        })?
}

fn prepare<'a>(
    record: &'a StoredDocument,
    source: &'a DocumentSourceReservation,
    edit: Option<(u64, &'a str)>,
) -> Result<Publication<'a>, DocumentError> {
    let metadata = encode(record)?;
    let reservation = object_row(source)?;
    if source.owner_hash != digest(record.metadata.owner_subject.as_bytes())
        || source.document_id != record.metadata.id
        || record.metadata.source_revision != record.metadata.revision
        || !references(record, record.metadata.revision, source)
    {
        return Err(DocumentError::Corrupt);
    }
    let edit = match edit {
        Some((expected, key)) if expected.checked_add(1) == Some(record.metadata.revision) => {
            let receipt = record.receipts.get(key).ok_or(DocumentError::Corrupt)?;
            if receipt.outcome.document != record.metadata {
                return Err(DocumentError::Corrupt);
            }
            Some((expected as i64, key, receipt.fingerprint.as_str()))
        }
        None if record.metadata.revision == 1 => None,
        _ => return Err(DocumentError::Corrupt),
    };
    Ok(Publication {
        record,
        metadata,
        source,
        reservation,
        marker: source_marker(source),
        edit,
    })
}

#[cfg(test)]
pub(super) async fn reconcile_after_majority_read(
    store: &MongoDocumentStore,
    record: &StoredDocument,
    source: &DocumentSourceReservation,
    edit: Option<(u64, &str)>,
) -> Result<bool, DocumentError> {
    let ledger = store.quota(true).rows;
    let mut session = ledger.client().start_session().await.unwrap();
    ledger
        .find_one(doc! { "_id": &source_marker(source)["_id"] })
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let time = session.operation_time().unwrap();
    prepare(record, source, edit)?
        .reconcile(&store.rows, &ledger, time)
        .await
}

pub(super) fn source_marker(source: &DocumentSourceReservation) -> Document {
    doc! {
        "_id": format!("source_commit:{}:{}:{}", source.owner_hash, source.document_id, source.sha256),
        "document_schema_version": 1_i64,
        "kind": "source_commit",
        "owner_hash": &source.owner_hash,
        "document_id": &source.document_id,
        "sha256": &source.sha256,
        "bytes": source.bytes as i64,
    }
}

fn references(record: &StoredDocument, revision: u64, source: &DocumentSourceReservation) -> bool {
    let key = source.object_key();
    record.revisions.iter().any(|candidate| {
        candidate.metadata.revision == revision
            && candidate.metadata.sha256 == source.sha256
            && candidate.metadata.size_bytes == source.bytes
            && candidate.object_key == key
    })
}

enum WriteError {
    Driver(mongodb::error::Error),
    Domain(DocumentError),
}

impl From<mongodb::error::Error> for WriteError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl Publication<'_> {
    async fn run(&self, store: &MongoDocumentStore) -> Result<bool, DocumentError> {
        let ledger = store.quota(true).rows;
        for _ in 0..ATTEMPTS {
            let mut session = store
                .rows
                .client()
                .start_session()
                .await
                .map_err(super::quota::unavailable)?;
            match start_transaction(&mut session).await {
                Ok(()) => {}
                Err(error) if is_confirmed_body_retry(&error) => continue,
                Err(_) => return Err(DocumentError::Unavailable),
            }
            let result = self.write(&store.rows, &ledger, &mut session).await;
            match result {
                Ok(true) => {}
                result => {
                    if session.abort_guarded().await.is_err() {
                        tracing::warn!("document source publication abort was not acknowledged");
                    }
                    match result {
                        Ok(applied) => return Ok(applied),
                        Err(WriteError::Driver(error)) if is_confirmed_body_retry(&error) => {
                            continue;
                        }
                        Err(WriteError::Driver(_)) => return Err(DocumentError::Unavailable),
                        Err(WriteError::Domain(error)) => return Err(error),
                    }
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(true),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) => {
                    tracing::debug!("reconciling an uncertain document source publication");
                    let time = session.operation_time().ok_or(DocumentError::Unavailable)?;
                    return self.reconcile(&store.rows, &ledger, time).await;
                }
                CommitOutcome::Failed(_) => return Err(DocumentError::Unavailable),
            }
        }
        tracing::warn!("document source publication exhausted transaction retries");
        Err(DocumentError::Unavailable)
    }

    async fn write(
        &self,
        rows: &Collection<Document>,
        ledger: &Collection<Document>,
        session: &mut ClientSession,
    ) -> Result<bool, WriteError> {
        // Establish the transaction on a read so cancelling its first command
        // cannot strand a lock on the Document.
        if ledger
            .find_one(doc! { "_id": &self.reservation["_id"] })
            .session(&mut *session)
            .await?
            .as_ref()
            != Some(&self.reservation)
        {
            return Err(WriteError::Domain(DocumentError::Corrupt));
        }
        if let Some((expected, _, _)) = self.edit {
            let result = rows
                .replace_one(
                    doc! {
                        "_id": &self.record.metadata.id,
                        "owner_subject": &self.record.metadata.owner_subject,
                        "revision": expected,
                    },
                    self.metadata.clone(),
                )
                .session(&mut *session)
                .await?;
            if result.matched_count == 0 {
                return Ok(false);
            }
        } else if let Err(error) = rows
            .insert_one(self.metadata.clone())
            .session(&mut *session)
            .await
        {
            if is_duplicate_key(&error) && !error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT)
            {
                return Ok(false);
            }
            return Err(error.into());
        }

        match ledger
            .find_one(doc! { "_id": &self.marker["_id"] })
            .session(&mut *session)
            .await?
        {
            Some(row) if row == self.marker => {}
            Some(_) => return Err(WriteError::Domain(DocumentError::Corrupt)),
            None => {
                // A future claim for an unpublished object must use this same identity.
                ledger
                    .insert_one(self.marker.clone())
                    .session(&mut *session)
                    .await?;
            }
        }
        Ok(true)
    }

    async fn reconcile(
        &self,
        rows: &Collection<Document>,
        ledger: &Collection<Document>,
        time: Timestamp,
    ) -> Result<bool, DocumentError> {
        let mut snapshot = rows
            .client()
            .start_session()
            .snapshot(true)
            .snapshot_time(time)
            .await
            .map_err(super::quota::unavailable)?;
        let current = rows
            .find_one(doc! { "_id": &self.record.metadata.id })
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .await
            .map_err(super::quota::unavailable)?
            .ok_or(DocumentError::Unavailable)
            .and_then(|row| decode(&row))?;
        let exact = match self.edit {
            Some((_, key, fingerprint)) => current.receipts.get(key).is_some_and(|receipt| {
                receipt.fingerprint == fingerprint
                    && receipt.outcome.document.revision == self.record.metadata.revision
            }),
            None => current.creation_fingerprint == self.record.creation_fingerprint,
        };
        if current.metadata.owner_subject != self.record.metadata.owner_subject
            || !exact
            || !references(&current, self.record.metadata.revision, self.source)
            || ledger
                .find_one(doc! { "_id": &self.marker["_id"] })
                .read_concern(ReadConcern::snapshot())
                .session(&mut snapshot)
                .await
                .map_err(super::quota::unavailable)?
                .as_ref()
                != Some(&self.marker)
        {
            return Err(DocumentError::Unavailable);
        }
        // Another attempt with the same key could have supplied the durable
        // response. Reload it through the normal creation or receipt replay.
        Ok(false)
    }
}
