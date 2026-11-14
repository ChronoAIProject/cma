use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
struct Queue {
    rows: Mutex<BTreeMap<usize, bool>>,
    rounds: AtomicUsize,
    judged: AtomicUsize,
    maximum_batch: AtomicUsize,
    leased: std::sync::atomic::AtomicBool,
}
impl Queue {
    fn add(&self, count: usize) {
        let mut rows = self.rows.lock().unwrap();
        let next = rows.last_key_value().map_or(0, |(index, _)| index + 1);
        rows.extend((next..next + count).map(|index| (index, false)));
    }
}
fn reservation(index: usize) -> DocumentSourceReservation {
    DocumentSourceReservation {
        owner_hash: digest(b"worker-fairness-owner"),
        document_id: "doc_00000000000000000000000000000001".into(),
        sha256: digest(&(index as u64).to_be_bytes()),
        bytes: 8,
    }
}
fn index(candidate: &ObjectCandidate) -> usize {
    uuid::Uuid::parse_str(candidate.reservation_id.as_deref().unwrap())
        .unwrap()
        .as_u128() as usize
        - 1
}
#[async_trait::async_trait]
impl DocumentReclaimStore for Queue {
    async fn ready(&self) -> Result<bool, DocumentError> {
        self.rounds.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
    async fn candidates(
        &self,
        before: i64,
        limit: usize,
    ) -> Result<Vec<ObjectCandidate>, DocumentError> {
        self.maximum_batch.fetch_max(limit, Ordering::SeqCst);
        let rows = self.rows.lock().unwrap();
        let condemned = rows
            .iter()
            .filter(|(_, condemned)| **condemned && !self.leased.load(Ordering::SeqCst));
        let reserved = rows.iter().filter(|(_, condemned)| !**condemned);
        Ok(condemned
            .take(limit)
            .chain(reserved.take(limit))
            .map(|(index, condemned)| ObjectCandidate {
                source: reservation(*index),
                reservation_id: Some(uuid::Uuid::from_u128(*index as u128 + 1).to_string()),
                reserved_before: before,
                condemned: *condemned,
            })
            .collect())
    }
    async fn judge(&self, candidate: &ObjectCandidate) -> Result<Judgement, DocumentError> {
        let mut rows = self.rows.lock().unwrap();
        let Some(condemned) = rows.get_mut(&index(candidate)) else {
            return Ok(Judgement::Skipped);
        };
        if *condemned {
            return Ok(Judgement::Skipped);
        }
        *condemned = true;
        self.judged.fetch_add(1, Ordering::SeqCst);
        Ok(Judgement::Condemned)
    }
    async fn claim_delete(
        &self,
        candidate: &ObjectCandidate,
        holder: &str,
        now: i64,
    ) -> Result<Option<DeleteClaim>, DocumentError> {
        Ok(Some(DeleteClaim {
            candidate: candidate.clone(),
            holder: holder.into(),
            fence: 1,
            lease_until: now + DELETE_LEASE.as_millis() as i64,
        }))
    }
    async fn finalize(&self, claim: &DeleteClaim) -> Result<bool, DocumentError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .remove(&index(&claim.candidate))
            .is_some())
    }
    async fn backlog(&self, _: i64) -> Result<ReclaimBacklog, DocumentError> {
        Ok(ReclaimBacklog {
            pending_objects: self.rows.lock().unwrap().len() as u64,
            oldest_age_seconds: 3600,
        })
    }
}
struct Dependency {
    calls: AtomicUsize,
    throttled: bool,
}
#[async_trait::async_trait]
impl DocumentBlobMaintenance for Dependency {
    async fn delete(&self, _: &str) -> Result<BlobDeletion, BlobMaintenanceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.throttled {
            Err(BlobMaintenanceError {
                error: DocumentError::Unavailable,
                retry_after: Some(Duration::from_secs(3600)),
            })
        } else {
            Ok(BlobDeletion::Absent)
        }
    }
    async fn list(&self, _: &str, _: Option<&str>) -> Result<BlobPage, BlobMaintenanceError> {
        unreachable!()
    }
    fn deletion_supported(&self) -> bool {
        true
    }
}
fn paced_worker(queue: Arc<Queue>, dependency: Arc<Dependency>) -> DocumentMaintenanceWorker {
    let mut worker =
        DocumentMaintenanceWorker::new(Some(DocumentMaintenance::new(queue, dependency, true)));
    worker.budget = crate::background::BackgroundBudget::new();
    worker
}
#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_discovers_maximum_owner_fixture_with_bounded_yields() {
    const OBJECTS: usize = 32_768;
    const ROUNDS: usize = OBJECTS / RECLAIM_BATCH;
    let old_pacing = RECLAIM_INTERVAL * (ROUNDS as u32 - 1);
    assert_eq!(old_pacing, Duration::from_secs(153_300));
    let mut keys = std::collections::BTreeSet::new();
    for index in 0..OBJECTS {
        let source = reservation(index);
        source.validate().unwrap();
        assert!(keys.insert(source.object_key()));
    }
    let queue = Arc::new(Queue::default());
    queue.add(OBJECTS);
    let dependency = Arc::new(Dependency {
        calls: 0.into(),
        throttled: false,
    });
    let (stop, signal) = tokio::sync::watch::channel(false);
    let started = tokio::time::Instant::now();
    let task = paced_worker(queue.clone(), dependency.clone()).spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(queue.judged.load(Ordering::SeqCst), RECLAIM_BATCH);
    // Running the real worker once per positive 250ms yield gives a finite,
    // deterministic mechanism check, not a production throughput promise.
    for _ in 1..ROUNDS {
        tokio::time::advance(Duration::from_millis(250)).await;
        settle().await;
    }
    let judged = queue.judged.load(Ordering::SeqCst);
    let rounds = queue.rounds.load(Ordering::SeqCst);
    let checkpoint = started.elapsed();
    if judged != OBJECTS && rounds == 1 {
        // Preserve the failing checkpoint while measuring the complete legacy
        // schedule with this same worker and paused clock.
        for round in 1..ROUNDS {
            let delay = if round == 1 {
                RECLAIM_INTERVAL - (checkpoint - Duration::from_secs(30))
            } else {
                RECLAIM_INTERVAL
            };
            tokio::time::advance(delay).await;
            settle().await;
        }
        eprintln!(
            "legacy drain observation: objects={}, rounds={}, deletes={}, elapsed={:?}",
            queue.judged.load(Ordering::SeqCst),
            queue.rounds.load(Ordering::SeqCst),
            dependency.calls.load(Ordering::SeqCst),
            started.elapsed()
        );
    }
    stop.send(true).unwrap();
    task.await.unwrap();
    eprintln!("maximum-owner fixture: objects={OBJECTS}, bytes_per_object=8, unique_valid_keys={}, rounds={rounds}, judged={judged}, checkpoint={checkpoint:?}; old minimum pacing={old_pacing:?} plus startup=30s", keys.len());
    assert_eq!(judged, OBJECTS);
    assert_eq!(rounds, ROUNDS);
    assert_eq!(dependency.calls.load(Ordering::SeqCst), OBJECTS);
    assert_eq!(queue.maximum_batch.load(Ordering::SeqCst), RECLAIM_BATCH);
}
#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_retry_after_blocks_deletes_but_allows_discovery() {
    let queue = Arc::new(Queue::default());
    queue.add(RECLAIM_BATCH);
    let dependency = Arc::new(Dependency {
        calls: 0.into(),
        throttled: true,
    });
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = paced_worker(queue.clone(), dependency.clone()).spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(queue.judged.load(Ordering::SeqCst), RECLAIM_BATCH);
    let calls = dependency.calls.load(Ordering::SeqCst);
    assert!(calls > 0 && calls <= RECLAIM_DELETE_CONCURRENCY);
    queue.add(RECLAIM_BATCH + 1);
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(250)).await;
        settle().await;
    }
    let judged = queue.judged.load(Ordering::SeqCst);
    let rounds = queue.rounds.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(299)).await;
    settle().await;
    assert_eq!(queue.rounds.load(Ordering::SeqCst), rounds);
    stop.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(judged, RECLAIM_BATCH * 2 + 1);
    assert_eq!(dependency.calls.load(Ordering::SeqCst), calls);
    eprintln!("retry window: judged={judged}, slices={rounds}, initial_deletes={calls}, additional_deletes=0");
}

#[tokio::test(start_paused = true)]
async fn document_reclaim_worker_active_lease_backlog_keeps_idle_pacing() {
    let queue = Arc::new(Queue::default());
    queue.add(RECLAIM_BATCH);
    for condemned in queue.rows.lock().unwrap().values_mut() {
        *condemned = true;
    }
    queue.leased.store(true, Ordering::SeqCst);
    let dependency = Arc::new(Dependency {
        calls: 0.into(),
        throttled: false,
    });
    let (stop, signal) = tokio::sync::watch::channel(false);
    let task = paced_worker(queue.clone(), dependency.clone()).spawn(signal);
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(queue.rounds.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(299)).await;
    settle().await;
    assert_eq!(queue.rounds.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(queue.rounds.load(Ordering::SeqCst), 2);
    assert_eq!(dependency.calls.load(Ordering::SeqCst), 0);
    assert_eq!(queue.judged.load(Ordering::SeqCst), 0);
    stop.send(true).unwrap();
    task.await.unwrap();
}
