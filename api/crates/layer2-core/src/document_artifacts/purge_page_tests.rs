use super::maintenance_tests::support::source;
use super::purge_tests::{drain, fixture, FIRST, SECOND};
use super::*;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::sync::Mutex;

#[tokio::test(start_paused = true)]
async fn purge_persists_listed_page_before_slow_delete_waves() {
    let (_, blobs, worker) = fixture(true, vec![]).await;
    for i in 0..130 {
        blobs.put(&source(i)).await;
    }
    blobs.delete_delay_ms.store(3000, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let mut deleted = 0;
    for _ in 0..150 {
        let now = FIRST + started.elapsed().as_millis() as i64;
        let next = tokio::time::timeout(Duration::from_secs(30), worker.purge_step(now))
            .await
            .unwrap()
            .unwrap();
        assert!(next.deleted <= 4);
        deleted += next.deleted;
        if next.claimed == 0 {
            break;
        }
    }
    assert_eq!(deleted, 130);
    let mut released = 0;
    for _ in 0..150 {
        let now = SECOND + started.elapsed().as_millis() as i64;
        let next = tokio::time::timeout(Duration::from_secs(30), worker.purge_step(now))
            .await
            .unwrap()
            .unwrap();
        released += next.released;
        if next.claimed == 0 {
            break;
        }
    }
    assert_eq!(released, 1);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 130);
    assert_eq!(blobs.maximum.load(Ordering::SeqCst), 4);
}

#[tokio::test(start_paused = true)]
async fn purge_without_ledger_pages_more_than_two_hundred_fifty_six_listed_objects() {
    let (_, blobs, worker) = fixture(true, vec![]).await;
    for i in 0..300 {
        blobs.put(&source(i)).await;
    }
    assert_eq!(drain(&worker, FIRST).await.deleted, 300);
    assert_eq!(drain(&worker, SECOND).await.released, 1);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 300);
    for i in 0..300 {
        assert!(!blobs.contains(&source(i)).await);
    }
}

#[derive(Default)]
struct OffsetPages {
    inner: memory::MemoryDocumentBlobs,
    cursors: Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl DocumentBlobMaintenance for OffsetPages {
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        self.inner.delete(key).await
    }

    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        if !valid_document_blob_prefix(prefix) {
            return Err(DocumentError::Invalid.into());
        }
        self.cursors.lock().await.push(cursor.map(str::to_owned));
        let tokens = ["page-Z", "page-A", "page-M", "page-B"];
        let offset = match cursor {
            None => 0,
            Some(value) => {
                (tokens
                    .iter()
                    .position(|token| *token == value)
                    .ok_or(DocumentError::Invalid)?
                    + 1)
                    * 2
            }
        };
        let rows = self.inner.0.lock().await;
        let keys: Vec<_> = rows
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        let next_offset = offset + 2;
        let next = if next_offset < keys.len() {
            Some(tokens[next_offset / 2 - 1].to_owned())
        } else {
            None
        };
        Ok(BlobPage {
            keys: keys.into_iter().skip(offset).take(2).rev().collect(),
            next,
        })
    }

    fn deletion_supported(&self) -> bool {
        true
    }
}

#[tokio::test(start_paused = true)]
async fn purge_verifies_opaque_offset_pages_and_finds_skipped_and_late_objects() {
    let (store, _, _) = fixture(true, vec![]).await;
    let blobs = Arc::new(OffsetPages::default());
    let reclaim = Arc::new(super::maintenance_tests::support::Store::new(false));
    let worker =
        DocumentMaintenance::new(reclaim, blobs.clone(), false).with_purge(store.clone(), true);
    drain(&worker, FIRST).await;
    blobs.cursors.lock().await.clear();
    for i in 0..8 {
        blobs
            .inner
            .put(&source(i).object_key(), vec![1; 8])
            .await
            .unwrap();
    }

    for _ in 0..6 {
        assert_eq!(worker.purge_step(SECOND).await.unwrap().released, 0);
    }
    assert_eq!(
        blobs.cursors.lock().await.as_slice(),
        &[None, Some("page-Z".into()), Some("page-A".into())]
    );
    assert_eq!(
        blobs.inner.0.lock().await.len(),
        4,
        "offsets skipped objects after earlier pages were deleted"
    );
    assert_eq!(store.backlog(SECOND).await.unwrap().pending_documents, 1);

    // A delayed source object appears after the cursor is exhausted.
    blobs
        .inner
        .put(&source(99).object_key(), vec![1; 8])
        .await
        .unwrap();
    assert_eq!(drain(&worker, SECOND).await.released, 1);
    assert!(blobs.inner.0.lock().await.is_empty());
    assert!(
        blobs
            .cursors
            .lock()
            .await
            .iter()
            .filter(|cursor| cursor.is_none())
            .count()
            >= 3
    );
    assert_eq!(store.backlog(SECOND).await.unwrap().pending_documents, 0);
}
