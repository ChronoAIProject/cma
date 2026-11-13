use super::maintenance_tests::support::{source, Blobs};
use super::*;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::sync::Mutex;

struct Row {
    document: PurgeClaimable,
    objects: Vec<String>,
    state: DocumentPurgeState,
    lease: Option<PurgeLease>,
    fence: u64,
    released: u64,
    advanced: u64,
    blocked_reason: Option<String>,
    now: i64,
}
pub(super) struct Store(Mutex<Row>);
impl Store {
    fn new(objects: Vec<String>) -> Self {
        Self(Mutex::new(Row {
            document: PurgeClaimable {
                id: source(0).document_id,
                owner_hash: source(0).owner_hash,
                deleted_at: 1000,
                last_reserved_at: Some(2000),
                pass: 1,
                next_at: 1000,
                position: None,
            },
            objects,
            state: DocumentPurgeState::Pending,
            lease: None,
            fence: 0,
            released: 0,
            advanced: 0,
            blocked_reason: None,
            now: 0,
        }))
    }
}
fn owns(row: &Row, lease: &PurgeLease) -> bool {
    row.state != DocumentPurgeState::Complete
        && row.lease.as_ref().is_some_and(|current| {
            current.holder == lease.holder
                && current.fence == lease.fence
                && current.expires_at > row.now
        })
}
#[async_trait::async_trait]
impl DocumentPurgeStore for Store {
    async fn due(&self, now: i64, limit: usize) -> Result<Vec<PurgeClaimable>, DocumentError> {
        let mut row = self.0.lock().await;
        row.now = row.now.max(now);
        Ok(
            if limit > 0 && row.state != DocumentPurgeState::Complete && row.document.next_at <= now
            {
                vec![row.document.clone()]
            } else {
                vec![]
            },
        )
    }
    async fn claim(
        &self,
        id: &str,
        holder: &str,
        now: i64,
    ) -> Result<Option<PurgeLease>, DocumentError> {
        let mut row = self.0.lock().await;
        row.now = row.now.max(now);
        let now = row.now;
        if id != row.document.id
            || row.state == DocumentPurgeState::Complete
            || row.document.next_at > now
            || row
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at >= now)
        {
            return Ok(None);
        }
        row.fence += 1;
        let lease = PurgeLease {
            document: row.document.clone(),
            holder: holder.into(),
            fence: row.fence,
            expires_at: now + PURGE_LEASE.as_millis() as i64,
        };
        row.lease = Some(lease.clone());
        Ok(Some(lease))
    }
    async fn objects(&self, lease: &PurgeLease) -> Result<Vec<String>, DocumentError> {
        let row = self.0.lock().await;
        Ok(if owns(&row, lease) {
            row.objects.clone()
        } else {
            vec![]
        })
    }
    async fn advance(&self, lease: &PurgeLease, step: PurgeStep) -> Result<bool, DocumentError> {
        let mut row = self.0.lock().await;
        if !owns(&row, lease) {
            return Ok(false);
        }
        match step {
            PurgeStep::AwaitSecondPass { next_at } => {
                row.document.pass = 2;
                row.document.next_at = next_at;
                row.document.position = None;
            }
            PurgeStep::Retry { next_at } => row.document.next_at = next_at,
            PurgeStep::Progress { position, next_at } => {
                row.document.position = Some(position);
                row.document.next_at = next_at;
            }
        }
        row.advanced += 1;
        row.lease = None;
        Ok(true)
    }
    async fn release(&self, lease: &PurgeLease) -> Result<bool, DocumentError> {
        let mut row = self.0.lock().await;
        if !owns(&row, lease) {
            return Ok(false);
        }
        row.released += 1;
        row.state = DocumentPurgeState::Complete;
        row.lease = None;
        Ok(true)
    }
    async fn block(&self, lease: &PurgeLease, reason: &str) -> Result<bool, DocumentError> {
        let mut row = self.0.lock().await;
        if !owns(&row, lease) {
            return Ok(false);
        }
        row.state = DocumentPurgeState::Blocked;
        row.blocked_reason = Some(reason.into());
        row.lease = None;
        Ok(true)
    }
    async fn backlog(&self, now: i64) -> Result<PurgeBacklog, DocumentError> {
        let row = self.0.lock().await;
        Ok(PurgeBacklog {
            pending_documents: u64::from(row.state != DocumentPurgeState::Complete),
            oldest_age_seconds: now.saturating_sub(row.document.deleted_at).max(0) as u64 / 1000,
        })
    }
}
fn maintenance(store: Arc<Store>, blobs: Arc<Blobs>) -> DocumentMaintenance {
    let reclaim = Arc::new(super::maintenance_tests::support::Store::new(false));
    DocumentMaintenance::new(reclaim, blobs, false).with_purge(store, true)
}
pub(super) async fn fixture(
    supported: bool,
    objects: Vec<String>,
) -> (Arc<Store>, Arc<Blobs>, DocumentMaintenance) {
    let store = Arc::new(Store::new(objects));
    let blobs = Arc::new(Blobs::new(supported));
    let worker = maintenance(store.clone(), blobs.clone());
    (store, blobs, worker)
}
pub(super) const FIRST: i64 = 3000;
pub(super) const SECOND: i64 = 2000 + 10 * 60 * 1000;

pub(super) async fn drain(worker: &DocumentMaintenance, now: i64) -> PurgeOutcome {
    let mut total = PurgeOutcome::default();
    for _ in 0..1024 {
        let next = worker.purge_step(now).await.unwrap();
        total.claimed += next.claimed;
        total.deleted += next.deleted;
        total.released += next.released;
        if next.claimed == 0 {
            return total;
        }
    }
    panic!("purge did not finish within its bounded fixture");
}

#[tokio::test(start_paused = true)]
async fn purge_deletes_snapshot_ledger_and_listed_objects_then_releases_once() {
    let (store, blobs, worker) =
        fixture(true, vec![source(1).object_key(), source(2).object_key()]).await;
    for i in 1..=3 {
        blobs.put(&source(i)).await;
    }
    let first = worker.purge_step(FIRST).await.unwrap();
    assert_eq!(first.deleted, 2);
    assert_eq!(first.released, 0);
    assert_eq!(drain(&worker, FIRST).await.deleted, 1);
    assert_eq!(store.0.lock().await.document.next_at, SECOND);
    assert_eq!(worker.purge_step(SECOND - 1).await.unwrap().claimed, 0);
    assert_eq!(drain(&worker, SECOND).await.released, 1);
    assert_eq!(worker.purge_step(SECOND).await.unwrap().released, 0);
    assert_eq!(store.0.lock().await.released, 1);
    assert_eq!(store.0.lock().await.state, DocumentPurgeState::Complete);
    for i in 1..=3 {
        assert!(!blobs.contains(&source(i)).await);
    }
}

#[tokio::test(start_paused = true)]
async fn purge_second_pass_removes_late_put_object() {
    let (store, blobs, worker) = fixture(true, vec![source(1).object_key()]).await;
    blobs.put(&source(1)).await;
    worker.purge_step(FIRST).await.unwrap();
    worker.purge_step(FIRST).await.unwrap();
    blobs.put(&source(9)).await;
    assert!(blobs.contains(&source(9)).await);
    drain(&worker, SECOND).await;
    assert!(!blobs.contains(&source(9)).await);
    assert_eq!(store.0.lock().await.released, 1);
}

#[tokio::test(start_paused = true)]
async fn purge_blocks_without_supported_deletion_and_never_claims_complete() {
    let (store, blobs, worker) = fixture(false, vec![source(1).object_key()]).await;
    blobs.put(&source(1)).await;
    let outcome = worker.purge_step(FIRST).await.unwrap();
    assert!(outcome.blocked);
    assert_eq!(outcome.blocked_reason, Some("storage_deletion_unsupported"));
    let row = store.0.lock().await;
    assert_eq!(row.state, DocumentPurgeState::Blocked);
    assert_eq!(
        row.blocked_reason.as_deref(),
        Some("storage_deletion_unsupported")
    );
    assert_eq!(row.advanced, 0);
    assert_eq!(row.released, 0);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn purge_storage_429_backs_off_and_resumes_same_step() {
    let (store, blobs, worker) = fixture(true, vec![source(1).object_key()]).await;
    blobs.put(&source(1)).await;
    blobs
        .fail(BlobMaintenanceError {
            error: DocumentError::Unavailable,
            retry_after: Some(Duration::from_secs(45)),
        })
        .await;
    let outcome = worker.purge_step(FIRST).await.unwrap();
    assert_eq!(outcome.retry_after, Some(Duration::from_secs(45)));
    assert_eq!(store.0.lock().await.document.pass, 1);
    assert_eq!(store.0.lock().await.document.next_at, FIRST + 45_000);
    assert_eq!(store.0.lock().await.released, 0);
    assert_eq!(worker.purge_step(FIRST + 44_999).await.unwrap().claimed, 0);
    worker.purge_step(FIRST + 45_000).await.unwrap();
    assert!(!blobs.contains(&source(1)).await);
}

#[tokio::test(start_paused = true)]
async fn purge_list_429_backs_off_without_advancing_the_position() {
    let (store, blobs, worker) = fixture(true, vec![]).await;
    blobs
        .fail_list(BlobMaintenanceError {
            error: DocumentError::Unavailable,
            retry_after: Some(Duration::from_secs(45)),
        })
        .await;
    let mut retried = false;
    for _ in 0..4 {
        let before = store.0.lock().await.document.position.clone();
        let outcome = worker.purge_step(FIRST).await.unwrap();
        if let Some(delay) = outcome.retry_after {
            assert_eq!(delay, Duration::from_secs(45));
            let row = store.0.lock().await;
            assert_eq!(row.document.position, before);
            assert_eq!(row.document.pass, 1);
            assert_eq!(row.document.next_at, FIRST + 45_000);
            retried = true;
            break;
        }
    }
    assert!(retried);
    assert_eq!(worker.purge_step(FIRST + 44_999).await.unwrap().claimed, 0);
    drain(&worker, FIRST + 45_000).await;
    assert_eq!(drain(&worker, SECOND).await.released, 1);
}

#[tokio::test(start_paused = true)]
async fn purge_never_touches_other_owner_or_other_document_prefix() {
    let (store, blobs, worker) = fixture(true, vec![source(1).object_key()]).await;
    let mut other_owner = source(2);
    other_owner.owner_hash = digest(b"other-owner");
    let mut other_doc = source(3);
    other_doc.document_id = "doc_00000000000000000000000000000002".into();
    for object in [&source(1), &other_owner, &other_doc] {
        blobs.put(object).await;
    }
    worker.purge_step(FIRST).await.unwrap();
    worker.purge_step(FIRST).await.unwrap();
    drain(&worker, SECOND).await;
    assert_eq!(store.0.lock().await.released, 1);
    assert!(blobs.contains(&other_owner).await);
    assert!(blobs.contains(&other_doc).await);
    let (store, blobs, worker) = fixture(true, vec![other_owner.object_key()]).await;
    blobs.put(&other_owner).await;
    assert_eq!(worker.purge_step(FIRST).await, Err(DocumentError::Corrupt));
    assert_eq!(store.0.lock().await.advanced, 0);
    assert!(blobs.contains(&other_owner).await);
}

#[tokio::test(start_paused = true)]
async fn purge_bounds_delete_concurrency_at_four() {
    let objects = (0..10).map(|i| source(i).object_key()).collect();
    let (_, blobs, worker) = fixture(true, objects).await;
    for i in 0..10 {
        blobs.put(&source(i)).await;
    }
    blobs.pause();
    let task = tokio::spawn(async move { worker.purge_step(FIRST).await });
    blobs.wait_for_calls(4).await;
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 4);
    blobs.resume(10);
    task.await.unwrap().unwrap();
    assert_eq!(blobs.maximum.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn expired_purge_worker_cannot_advance_or_release_after_new_claim() {
    let store = Store::new(vec![]);
    let id = source(0).document_id;
    let old = store.claim(&id, "old", FIRST).await.unwrap().unwrap();
    let new = store
        .claim(&id, "new", old.expires_at + 1)
        .await
        .unwrap()
        .unwrap();
    assert!(!store
        .advance(&old, PurgeStep::AwaitSecondPass { next_at: SECOND })
        .await
        .unwrap());
    assert!(!store.release(&old).await.unwrap());
    assert!(store.release(&new).await.unwrap());
    assert!(!store.release(&new).await.unwrap());
    assert_eq!(store.0.lock().await.released, 1);
}

#[tokio::test]
async fn expired_purge_worker_cannot_mutate_without_a_replacement_claim() {
    let store = Store::new(vec![source(1).object_key()]);
    let id = source(0).document_id;
    let lease = store.claim(&id, "original", FIRST).await.unwrap().unwrap();
    store.0.lock().await.now = lease.expires_at;
    assert!(store.objects(&lease).await.unwrap().is_empty());
    assert!(!store
        .advance(&lease, PurgeStep::Retry { next_at: SECOND })
        .await
        .unwrap());
    assert!(!store.release(&lease).await.unwrap());
    assert!(!store
        .block(&lease, "storage_deletion_unsupported")
        .await
        .unwrap());
    let row = store.0.lock().await;
    assert_eq!(
        row.fence, lease.fence,
        "the original claim remains unreplaced"
    );
    assert_eq!(row.lease.as_ref().unwrap().holder, "original");
    assert_eq!(row.state, DocumentPurgeState::Pending);
    assert_eq!(row.released, 0);
    assert_eq!(row.advanced, 0);
    assert_eq!(row.blocked_reason, None);
}

#[tokio::test(start_paused = true)]
async fn purge_persists_bounded_progress_across_more_than_sixty_four_known_objects() {
    let objects = (0..130).map(|i| source(i).object_key()).collect();
    let (store, blobs, worker) = fixture(true, objects).await;
    for i in 0..130 {
        blobs.put(&source(i)).await;
    }
    assert_eq!(worker.purge_step(FIRST).await.unwrap().deleted, 4);
    assert!(matches!(
        store.0.lock().await.document.position,
        Some(PurgePosition::KnownObjects { .. })
    ));
    assert_eq!(drain(&worker, FIRST).await.deleted, 126);
    assert_eq!(store.0.lock().await.document.pass, 2);
    assert_eq!(drain(&worker, SECOND).await.released, 1);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 260);
}

#[tokio::test(start_paused = true)]
async fn purge_persists_each_slow_delete_wave_within_the_worker_budget() {
    let objects = (0..130).map(|i| source(i).object_key()).collect();
    let (store, blobs, worker) = fixture(true, objects).await;
    for i in 0..130 {
        blobs.put(&source(i)).await;
    }
    blobs.delete_delay_ms.store(3000, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let first = tokio::time::timeout(Duration::from_secs(30), worker.purge_step(FIRST)).await;
    assert!(
        first.is_ok(),
        "a completed delete wave must persist before the worker budget expires"
    );
    assert_eq!(first.unwrap().unwrap().deleted, 4);
    for _ in 0..150 {
        if store.0.lock().await.document.pass == 2 {
            break;
        }
        let now = FIRST + started.elapsed().as_millis() as i64;
        tokio::time::timeout(Duration::from_secs(30), worker.purge_step(now))
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(store.0.lock().await.document.pass, 2);
    for _ in 0..150 {
        if store.0.lock().await.state == DocumentPurgeState::Complete {
            break;
        }
        let now = SECOND + started.elapsed().as_millis() as i64;
        tokio::time::timeout(Duration::from_secs(30), worker.purge_step(now))
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(store.0.lock().await.released, 1);
    assert_eq!(blobs.calls.load(Ordering::SeqCst), 260);
}
