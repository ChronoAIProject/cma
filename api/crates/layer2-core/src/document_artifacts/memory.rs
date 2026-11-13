//! Deterministic stores for domain and public-interface conformance tests only.
use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

#[path = "memory_quota.rs"]
mod quota;
pub use quota::MemoryDocumentQuota;

#[derive(Default)]
pub struct MemoryDocuments(
    pub Mutex<BTreeMap<String, StoredDocument>>,
    Mutex<BTreeMap<String, MemorySourceState>>,
    Mutex<BTreeMap<String, (DocumentTombstone, DocumentPurgeState)>>,
    AtomicUsize,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemorySourceState {
    Reserved,
    Committed,
    Condemned,
}

impl MemoryDocuments {
    pub fn lifecycle_reads(&self) -> usize {
        self.3.load(Ordering::SeqCst)
    }

    pub async fn set_purge_state(&self, id: &str, state: DocumentPurgeState) {
        self.2.lock().await.get_mut(id).expect("deleted document").1 = state;
    }

    pub async fn condemn_source(&self, source: &DocumentSourceReservation) {
        self.1
            .lock()
            .await
            .insert(source.object_key(), MemorySourceState::Condemned);
    }

    pub async fn source_state(&self, source: &DocumentSourceReservation) -> Option<&'static str> {
        match self.1.lock().await.get(&source.object_key()).copied() {
            Some(MemorySourceState::Reserved) => Some("reserved"),
            Some(MemorySourceState::Committed) => Some("committed"),
            Some(MemorySourceState::Condemned) => Some("condemned"),
            None => None,
        }
    }
}

async fn commit_source(
    states: &mut BTreeMap<String, MemorySourceState>,
    source: &DocumentSourceReservation,
) -> Result<(), DocumentError> {
    match states.get(&source.object_key()).copied() {
        Some(MemorySourceState::Condemned) => Err(DocumentError::Unavailable),
        Some(MemorySourceState::Reserved | MemorySourceState::Committed) => {
            states.insert(source.object_key(), MemorySourceState::Committed);
            Ok(())
        }
        None => {
            states.insert(source.object_key(), MemorySourceState::Committed);
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl DocumentStore for MemoryDocuments {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        Ok(self.0.lock().await.get(id).cloned())
    }
    async fn insert(
        &self,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<InsertOutcome, DocumentError> {
        let mut rows = self.0.lock().await;
        let tombstones = self.2.lock().await;
        if tombstones.contains_key(&record.metadata.id) {
            return Ok(InsertOutcome::Deleted);
        }
        if rows.contains_key(&record.metadata.id) {
            return Ok(InsertOutcome::Exists);
        }
        let mut states = self.1.lock().await;
        if let Some((source, SourceTicket::Ledgered)) = source {
            states
                .entry(source.object_key())
                .or_insert(MemorySourceState::Reserved);
            commit_source(&mut states, source).await?;
        }
        rows.insert(record.metadata.id.clone(), record.clone());
        Ok(InsertOutcome::Inserted)
    }
    async fn delete(
        &self,
        expected: u64,
        tombstone: &DocumentTombstone,
    ) -> Result<DeleteOutcome, DocumentError> {
        tombstone.validate()?;
        let mut rows = self.0.lock().await;
        let mut tombstones = self.2.lock().await;
        if let Some((previous, _)) = tombstones.get(&tombstone.id) {
            return Ok(DeleteOutcome::AlreadyDeleted(previous.clone()));
        }
        let Some(record) = rows.get(&tombstone.id) else {
            return Ok(DeleteOutcome::NotFound);
        };
        if digest(record.metadata.owner_subject.as_bytes()) != tombstone.owner_hash {
            return Ok(DeleteOutcome::NotFound);
        }
        if record.metadata.revision != expected {
            return Ok(DeleteOutcome::Conflict);
        }
        tombstones.insert(
            tombstone.id.clone(),
            (tombstone.clone(), DocumentPurgeState::Pending),
        );
        rows.remove(&tombstone.id);
        Ok(DeleteOutcome::Deleted)
    }
    async fn tombstone(&self, id: &str) -> Result<Option<DocumentTombstone>, DocumentError> {
        self.3.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .2
            .lock()
            .await
            .get(id)
            .map(|(tombstone, _)| tombstone.clone()))
    }
    async fn purge_state(&self, id: &str) -> Result<Option<DocumentPurgeState>, DocumentError> {
        self.3.fetch_add(1, Ordering::SeqCst);
        Ok(self.2.lock().await.get(id).map(|(_, state)| *state))
    }
    async fn pending_deletions(&self, owner_hash: &str) -> Result<u64, DocumentError> {
        self.3.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .2
            .lock()
            .await
            .values()
            .filter(|(tombstone, state)| {
                tombstone.owner_hash == owner_hash && *state != DocumentPurgeState::Complete
            })
            .count() as u64)
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<bool, DocumentError> {
        let mut rows = self.0.lock().await;
        if !rows.get(&record.metadata.id).is_some_and(|r| {
            r.metadata.revision == expected
                && r.metadata.owner_subject == record.metadata.owner_subject
        }) {
            return Ok(false);
        }
        let mut states = self.1.lock().await;
        if let Some((source, SourceTicket::Ledgered)) = source {
            states
                .entry(source.object_key())
                .or_insert(MemorySourceState::Reserved);
            commit_source(&mut states, source).await?;
        }
        rows.insert(record.metadata.id.clone(), record.clone());
        Ok(true)
    }
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        query.validate()?;
        Ok(self
            .0
            .lock()
            .await
            .values()
            .filter(|r| {
                let owned = r.metadata.owner_subject == subject;
                let shared = r.shares.iter().any(|s| s.subject == subject);
                let included = match query.scope.as_deref().unwrap_or("all") {
                    "owned" => owned,
                    "shared" => !owned && shared,
                    _ => owned || shared,
                };
                included
                    && query.cursor.as_ref().is_none_or(|v| &r.metadata.id > v)
                    && query
                        .q
                        .as_ref()
                        .is_none_or(|q| r.title_search.contains(&q.to_lowercase()))
            })
            .take(query.limit() + 1)
            .map(|r| DocumentListing {
                metadata: r.metadata.clone(),
                shares: r.shares.clone(),
            })
            .collect())
    }
}
#[derive(Default)]
pub struct MemoryDocumentBlobs(pub Mutex<BTreeMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl DocumentBlobStore for MemoryDocumentBlobs {
    async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), DocumentError> {
        let mut rows = self.0.lock().await;
        if rows.get(key).is_some_and(|v| v != &bytes) {
            return Err(DocumentError::Corrupt);
        }
        rows.insert(key.into(), bytes);
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, DocumentError> {
        self.0
            .lock()
            .await
            .get(key)
            .cloned()
            .ok_or(DocumentError::Unavailable)
    }
}

#[async_trait::async_trait]
impl DocumentBlobMaintenance for MemoryDocumentBlobs {
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        let valid = key.rsplit_once('/').is_some_and(|(prefix, file)| {
            valid_document_blob_prefix(&format!("{prefix}/"))
                && prefix.split('/').count() == 3
                && file.strip_suffix(".md").is_some_and(valid_digest)
        });
        if !valid {
            return Err(DocumentError::Invalid.into());
        }
        Ok(if self.0.lock().await.remove(key).is_some() {
            BlobDeletion::Deleted
        } else {
            BlobDeletion::Absent
        })
    }
    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        if !valid_document_blob_prefix(prefix)
            || cursor.is_some_and(|value| value.len() > MAX_BLOB_CURSOR_BYTES)
        {
            return Err(DocumentError::Invalid.into());
        }
        let rows = self.0.lock().await;
        let mut keys = rows
            .keys()
            .filter(|key| {
                key.starts_with(prefix) && cursor.is_none_or(|cursor| key.as_str() > cursor)
            })
            .take(MAX_BLOB_PAGE_KEYS + 1)
            .cloned()
            .collect::<Vec<_>>();
        let next = if keys.len() > MAX_BLOB_PAGE_KEYS {
            keys.pop();
            keys.last().cloned()
        } else {
            None
        };
        Ok(BlobPage { keys, next })
    }
    fn deletion_supported(&self) -> bool {
        true
    }
}
