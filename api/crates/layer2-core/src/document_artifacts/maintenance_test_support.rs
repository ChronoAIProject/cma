use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Mutex, Notify, Semaphore};

pub const NOW: i64 = 3_600_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Reserved,
    Baseline,
    Committed,
    Condemned,
}
struct Row {
    source: DocumentSourceReservation,
    state: State,
    reserved_at: i64,
}
#[derive(Default)]
struct Data {
    rows: BTreeMap<String, Row>,
    references: BTreeSet<String>,
    documents: BTreeSet<String>,
    counters: [i64; 3],
}
pub struct Store {
    ready: bool,
    data: Mutex<Data>,
    pub reads: AtomicUsize,
    pub judgements: AtomicUsize,
}
impl Store {
    pub fn new(ready: bool) -> Self {
        Self {
            ready,
            data: Mutex::new(Data::default()),
            reads: 0.into(),
            judgements: 0.into(),
        }
    }
    pub async fn reserve(&self, source: DocumentSourceReservation, reserved_at: i64) {
        let mut data = self.data.lock().await;
        if !data
            .rows
            .values()
            .any(|row| row.source.document_id == source.document_id)
        {
            data.counters[0] += 1;
        }
        data.counters[1] += source.bytes as i64;
        data.counters[2] += 1;
        data.rows.insert(
            source.object_key(),
            Row {
                source,
                state: State::Reserved,
                reserved_at,
            },
        );
    }
    pub async fn reference(&self, source: &DocumentSourceReservation) {
        let mut data = self.data.lock().await;
        data.references.insert(source.object_key());
        data.documents.insert(source.document_id.clone());
    }
    pub async fn set_state(&self, source: &DocumentSourceReservation, state: State) {
        self.data
            .lock()
            .await
            .rows
            .get_mut(&source.object_key())
            .unwrap()
            .state = state;
    }
    pub async fn state(&self, source: &DocumentSourceReservation) -> Option<State> {
        self.data
            .lock()
            .await
            .rows
            .get(&source.object_key())
            .map(|row| row.state)
    }
    pub async fn counters(&self) -> [i64; 3] {
        self.data.lock().await.counters
    }
    pub async fn corrupt_counters(&self, counters: [i64; 3]) {
        self.data.lock().await.counters = counters;
    }
}
#[async_trait::async_trait]
impl DocumentReclaimStore for Store {
    async fn ready(&self) -> Result<bool, DocumentError> {
        Ok(self.ready)
    }
    async fn candidates(
        &self,
        reserved_before: i64,
        limit: usize,
    ) -> Result<Vec<ObjectCandidate>, DocumentError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let data = self.data.lock().await;
        let mut rows: Vec<_> = data
            .rows
            .values()
            .filter(|row| {
                row.state == State::Condemned
                    || (matches!(row.state, State::Reserved | State::Baseline)
                        && row.reserved_at < reserved_before)
            })
            .collect();
        rows.sort_by_key(|row| row.state != State::Condemned);
        Ok(rows
            .into_iter()
            .take(limit)
            .map(|row| ObjectCandidate {
                source: row.source.clone(),
                reservation_id: None,
                reserved_before,
                condemned: row.state == State::Condemned,
            })
            .collect())
    }
    async fn judge(&self, candidate: &ObjectCandidate) -> Result<Judgement, DocumentError> {
        self.judgements.fetch_add(1, Ordering::SeqCst);
        let mut data = self.data.lock().await;
        let key = candidate.source.object_key();
        let referenced = data.references.contains(&key);
        let Some(row) = data.rows.get_mut(&key) else {
            return Ok(Judgement::Skipped);
        };
        if !matches!(row.state, State::Reserved | State::Baseline)
            || row.reserved_at >= candidate.reserved_before
        {
            return Ok(Judgement::Skipped);
        }
        row.state = if referenced {
            State::Committed
        } else {
            State::Condemned
        };
        Ok(if referenced {
            Judgement::Committed
        } else {
            Judgement::Condemned
        })
    }
    async fn finalize(&self, candidate: &ObjectCandidate) -> Result<bool, DocumentError> {
        let mut data = self.data.lock().await;
        let key = candidate.source.object_key();
        let Some(row) = data.rows.get(&key) else {
            return Ok(false);
        };
        if row.state != State::Condemned {
            return Ok(false);
        }
        let document = row.source.document_id.clone();
        let release_document = !data.documents.contains(&document)
            && data
                .rows
                .values()
                .filter(|row| row.source.document_id == document)
                .count()
                == 1;
        let deductions = [i64::from(release_document), row.source.bytes as i64, 1];
        let mut counters = data.counters;
        for (counter, deduction) in counters.iter_mut().zip(deductions) {
            *counter = counter
                .checked_sub(deduction)
                .filter(|n| *n >= 0)
                .ok_or(DocumentError::Corrupt)?;
        }
        data.rows.remove(&key);
        data.counters = counters;
        Ok(true)
    }
    async fn backlog(&self, now: i64) -> Result<ReclaimBacklog, DocumentError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let data = self.data.lock().await;
        let pending: Vec<_> = data
            .rows
            .values()
            .filter(|row| {
                row.state == State::Condemned
                    || (matches!(row.state, State::Reserved | State::Baseline)
                        && row.reserved_at < now - RECLAIM_GRACE.as_millis() as i64)
            })
            .collect();
        Ok(ReclaimBacklog {
            pending_objects: pending.len() as u64,
            oldest_age_seconds: pending
                .iter()
                .map(|row| now.saturating_sub(row.reserved_at).max(0) as u64 / 1000)
                .max()
                .unwrap_or(0),
        })
    }
}
pub struct Blobs {
    inner: memory::MemoryDocumentBlobs,
    supported: bool,
    failure: Mutex<Option<BlobMaintenanceError>>,
    pub calls: AtomicUsize,
    active: AtomicUsize,
    pub maximum: AtomicUsize,
    paused: AtomicBool,
    changed: Notify,
    permits: Semaphore,
}
impl Blobs {
    pub fn new(supported: bool) -> Self {
        Self {
            inner: Default::default(),
            supported,
            failure: Mutex::new(None),
            calls: 0.into(),
            active: 0.into(),
            maximum: 0.into(),
            paused: false.into(),
            changed: Notify::new(),
            permits: Semaphore::new(0),
        }
    }
    pub async fn put(&self, source: &DocumentSourceReservation) {
        self.inner
            .put(&source.object_key(), vec![1; source.bytes as usize])
            .await
            .unwrap();
    }
    pub async fn contains(&self, source: &DocumentSourceReservation) -> bool {
        self.inner.0.lock().await.contains_key(&source.object_key())
    }
    pub async fn fail(&self, error: BlobMaintenanceError) {
        *self.failure.lock().await = Some(error);
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self, permits: usize) {
        self.permits.add_permits(permits);
    }
    pub async fn wait_for_calls(&self, wanted: usize) {
        loop {
            let changed = self.changed.notified();
            if self.calls.load(Ordering::SeqCst) >= wanted {
                return;
            }
            changed.await;
        }
    }
}
#[async_trait::async_trait]
impl DocumentBlobMaintenance for Blobs {
    async fn delete(&self, key: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        self.changed.notify_waiters();
        if self.paused.load(Ordering::SeqCst) {
            self.permits.acquire().await.unwrap().forget();
        }
        let result = match self.failure.lock().await.take() {
            Some(error) => Err(error),
            None => self.inner.delete(key).await,
        };
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
    async fn list(
        &self,
        prefix: &str,
        cursor: Option<&str>,
    ) -> Result<BlobPage, BlobMaintenanceError> {
        self.inner.list(prefix, cursor).await
    }
    fn deletion_supported(&self) -> bool {
        self.supported
    }
}
pub fn source(index: usize) -> DocumentSourceReservation {
    DocumentSourceReservation {
        owner_hash: digest(b"maintenance-owner"),
        document_id: "doc_00000000000000000000000000000001".into(),
        sha256: digest(index.to_string().as_bytes()),
        bytes: 8,
    }
}
pub fn fixture() -> (Arc<Store>, Arc<Blobs>, DocumentMaintenance) {
    let store = Arc::new(Store::new(true));
    let blobs = Arc::new(Blobs::new(true));
    let maintenance = DocumentMaintenance::new(store.clone(), blobs.clone(), true);
    (store, blobs, maintenance)
}
