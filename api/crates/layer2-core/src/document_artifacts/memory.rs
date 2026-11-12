//! Deterministic stores for domain and public-interface conformance tests only.
use super::*;
use std::collections::BTreeMap;
use tokio::sync::Mutex;

#[path = "memory_quota.rs"]
mod quota;
pub use quota::MemoryDocumentQuota;

#[derive(Default)]
pub struct MemoryDocuments(
    pub Mutex<BTreeMap<String, StoredDocument>>,
    Mutex<BTreeMap<String, MemorySourceState>>,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemorySourceState {
    Reserved,
    Committed,
    Condemned,
}

impl MemoryDocuments {
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
    ) -> Result<bool, DocumentError> {
        let mut rows = self.0.lock().await;
        if rows.contains_key(&record.metadata.id) {
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
