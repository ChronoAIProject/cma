//! Deterministic stores for domain and public-interface conformance tests only.
use super::*;
use crate::{
    DirectoryQuery, Page, PublicIdentity, RegisteredUser, StoreError, SubjectId, UserDirectory,
    UserDirectoryStore,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;

/// Explicit registered subjects for document sharing tests.
pub struct MemoryRecipientDirectory {
    subjects: Vec<SubjectId>,
    pub unavailable: AtomicBool,
}

impl MemoryRecipientDirectory {
    pub fn new(subjects: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            subjects: subjects
                .iter()
                .map(|subject| SubjectId::new(*subject))
                .collect(),
            unavailable: AtomicBool::new(false),
        })
    }

    fn check_available(&self) -> Result<(), StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(StoreError::Unavailable("test directory unavailable".into()))
        } else {
            Ok(())
        }
    }

    fn record(subject: &SubjectId) -> RegisteredUser {
        RegisteredUser {
            identity: PublicIdentity {
                subject: subject.clone(),
                display_name: Some(format!("Name {subject}")),
                avatar_url: Some("https://example.invalid/avatar".into()),
            },
            first_seen_at: "2026-10-22T00:00:00Z".into(),
            last_seen_at: "2026-10-22T00:00:00Z".into(),
        }
    }
}

pub fn recipient_directory(subjects: &[&str]) -> Arc<UserDirectory> {
    UserDirectory::new(MemoryRecipientDirectory::new(subjects))
}

#[async_trait::async_trait]
impl UserDirectoryStore for MemoryRecipientDirectory {
    async fn observe(&self, _: &PublicIdentity) -> Result<(), StoreError> {
        Err(StoreError::Unavailable(
            "test directory is read-only".into(),
        ))
    }

    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        self.check_available()?;
        Ok(self
            .subjects
            .contains(subject)
            .then(|| Self::record(subject)))
    }

    async fn list(&self, query: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        self.check_available()?;
        let items = self
            .subjects
            .iter()
            .map(Self::record)
            .filter(|record| {
                query.text.as_ref().is_none_or(|text| {
                    record.identity.subject.as_str().contains(text)
                        || record
                            .identity
                            .display_name
                            .as_ref()
                            .is_some_and(|name| name.contains(text))
                })
            })
            .take(query.page.limit as usize)
            .collect();
        Ok(Page {
            items,
            next_cursor: None,
        })
    }
}

#[derive(Default)]
pub struct MemoryDocuments(
    pub Mutex<BTreeMap<String, StoredDocument>>,
    Mutex<BTreeMap<String, (DocumentTombstone, StoredDocument)>>,
);
#[async_trait::async_trait]
impl DocumentStore for MemoryDocuments {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        Ok(self.0.lock().await.get(id).cloned())
    }
    async fn tombstone(&self, id: &str) -> Result<Option<DocumentTombstone>, DocumentError> {
        Ok(self
            .1
            .lock()
            .await
            .get(id)
            .map(|(tombstone, _)| tombstone.clone()))
    }
    async fn delete(
        &self,
        owner: &str,
        tombstone: &DocumentTombstone,
    ) -> Result<bool, DocumentError> {
        tombstone.validate()?;
        let mut rows = self.0.lock().await;
        let mut deleted = self.1.lock().await;
        if tombstone.owner_hash != digest(owner.as_bytes()) {
            return Err(DocumentError::Corrupt);
        }
        let id = &tombstone.receipt.id;
        if !rows.get(id).is_some_and(|row| {
            row.metadata.owner_subject == owner
                && row.metadata.revision == tombstone.expected_revision
        }) {
            return Ok(false);
        }
        let audit = rows.remove(id).ok_or(DocumentError::Corrupt)?;
        deleted.insert(id.clone(), (tombstone.clone(), audit));
        Ok(true)
    }
    async fn insert(&self, record: &StoredDocument) -> Result<bool, DocumentError> {
        let mut rows = self.0.lock().await;
        if rows.contains_key(&record.metadata.id)
            || self.1.lock().await.contains_key(&record.metadata.id)
        {
            return Ok(false);
        }
        rows.insert(record.metadata.id.clone(), record.clone());
        Ok(true)
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
    ) -> Result<bool, DocumentError> {
        let mut rows = self.0.lock().await;
        if !rows.get(&record.metadata.id).is_some_and(|r| {
            r.metadata.revision == expected
                && r.metadata.owner_subject == record.metadata.owner_subject
        }) {
            return Ok(false);
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
