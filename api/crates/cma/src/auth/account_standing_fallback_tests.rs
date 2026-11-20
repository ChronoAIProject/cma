use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Barrier, Notify, Semaphore};

struct FrameSource {
    result: RwLock<Result<bool, AccountDeletionError>>,
    point_reads: AtomicUsize,
    resources: AtomicUsize,
    paused: AtomicBool,
    entered: Notify,
    release: Semaphore,
}
struct PointResource<'a>(&'a AtomicUsize);
impl Drop for PointResource<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl SnapshotSource for FrameSource {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        Ok(BTreeSet::new())
    }
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.resources.fetch_add(1, Ordering::SeqCst);
        let _resource = PointResource(&self.resources);
        self.point_reads.fetch_add(1, Ordering::SeqCst);
        let result = self.result.read().unwrap().clone();
        self.entered.notify_one();
        if self.paused.load(Ordering::SeqCst) {
            self.release.acquire().await.unwrap().forget();
        }
        result
    }
}

fn fixture(
    result: Result<bool, AccountDeletionError>,
    paused: bool,
) -> (Arc<FrameSource>, Arc<ErasingSnapshot>) {
    let source = Arc::new(FrameSource {
        result: RwLock::new(result),
        point_reads: AtomicUsize::new(0),
        resources: AtomicUsize::new(0),
        paused: AtomicBool::new(paused),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let gate = Arc::new(ErasingSnapshot::with_source(source.clone()));
    gate.state.write().unwrap().refreshed_at = Some(Instant::now() - Duration::from_secs(11));
    (source, gate)
}

async fn concurrent_frames_read_independently(result: Result<bool, AccountDeletionError>) {
    const FRAMES: usize = 32;
    let (source, gate) = fixture(result.clone(), true);
    let barrier = Arc::new(Barrier::new(FRAMES + 1));
    let mut frames = tokio::task::JoinSet::new();
    for _ in 0..FRAMES {
        let gate = gate.clone();
        let barrier = barrier.clone();
        frames.spawn(async move {
            barrier.wait().await;
            gate.erasing(&SubjectId::new("owner")).await
        });
    }
    barrier.wait().await;
    source.entered.notified().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while source.point_reads.load(Ordering::SeqCst) < FRAMES {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(source.point_reads.load(Ordering::SeqCst), FRAMES);
    source.release.add_permits(FRAMES);
    while let Some(frame) = frames.join_next().await {
        assert_eq!(frame.unwrap(), result);
    }
    assert_eq!(source.point_reads.load(Ordering::SeqCst), FRAMES);
}

#[tokio::test]
async fn concurrent_allowed_frames_require_independent_reads() {
    concurrent_frames_read_independently(Ok(false)).await;
}

#[tokio::test]
async fn concurrent_denied_frames_keep_their_own_pending_reads() {
    concurrent_frames_read_independently(Ok(true)).await;
}

#[tokio::test]
async fn concurrent_source_failures_do_not_allow_frames() {
    concurrent_frames_read_independently(Err(AccountDeletionError::Unavailable)).await;
}

#[tokio::test(start_paused = true)]
async fn stale_fallback_denial_cache_expires_by_the_next_refresh_period() {
    let (source, gate) = fixture(Ok(true), false);
    let owner = SubjectId::new("owner");
    assert!(gate.erasing(&owner).await.unwrap());
    for _ in 0..64 {
        assert!(gate.erasing(&owner).await.unwrap());
    }
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
    *source.result.write().unwrap() = Ok(false);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(!gate.erasing(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_does_not_cache_allow_across_a_source_outage() {
    let (source, gate) = fixture(Ok(false), false);
    let owner = SubjectId::new("owner");
    assert!(!gate.erasing(&owner).await.unwrap());
    *source.result.write().unwrap() = Err(AccountDeletionError::Unavailable);
    assert_eq!(
        gate.erasing(&owner).await,
        Err(AccountDeletionError::Unavailable)
    );
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_denial_is_scoped_to_the_subject_hash() {
    let (source, gate) = fixture(Ok(true), false);
    let owner = SubjectId::new("owner");
    assert!(gate.erasing(&owner).await.unwrap());
    *source.result.write().unwrap() = Ok(false);
    assert!(!gate.erasing(&SubjectId::new("other-owner")).await.unwrap());
    assert!(gate.erasing(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_local_acceptance_preempts_an_inflight_allow() {
    let (source, gate) = fixture(Ok(false), true);
    let owner = SubjectId::new("owner");
    let checking = {
        let gate = gate.clone();
        let owner = owner.clone();
        tokio::spawn(async move { gate.erasing(&owner).await })
    };
    source.entered.notified().await;
    gate.insert_local(&subject_hash(&owner));
    source.release.add_permits(1);
    assert!(checking.await.unwrap().unwrap());
    *source.result.write().unwrap() = Err(AccountDeletionError::Unavailable);
    assert!(gate.erasing(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_fallback_a_fresh_allow_cannot_weaken_an_inflight_authority_refusal() {
    for result in [Ok(true), Err(AccountDeletionError::Unavailable)] {
        let (source, gate) = fixture(result.clone(), true);
        let checking = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
        };
        source.entered.notified().await;
        gate.refresh().await.unwrap();
        source.release.add_permits(1);
        assert_eq!(checking.await.unwrap(), result);
    }
}

#[tokio::test]
async fn cancelling_one_frame_preserves_another_frames_independent_read() {
    let (source, gate) = fixture(Ok(true), true);
    let first = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
    };
    source.entered.notified().await;
    let next = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
    };
    source.entered.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(source.resources.load(Ordering::SeqCst), 1);
    source.release.add_permits(1);
    assert!(tokio::time::timeout(Duration::from_secs(1), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
    assert_eq!(source.resources.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn stale_fallback_restarts_an_abandoned_lookup_for_the_same_subject() {
    let (source, gate) = fixture(Ok(false), true);
    let owner = SubjectId::new("owner");
    let first = {
        let gate = gate.clone();
        let owner = owner.clone();
        tokio::spawn(async move { gate.erasing(&owner).await })
    };
    source.entered.notified().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    *source.result.write().unwrap() = Ok(true);
    source.paused.store(false, Ordering::SeqCst);
    source.release.add_permits(1);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(gate.erasing(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_cancelling_all_frames_releases_the_point_read_resource_immediately() {
    let (source, gate) = fixture(Ok(false), true);
    let checking = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
    };
    source.entered.notified().await;
    assert_eq!(source.resources.load(Ordering::SeqCst), 1);
    checking.abort();
    assert!(checking.await.unwrap_err().is_cancelled());
    assert_eq!(source.resources.load(Ordering::SeqCst), 0);
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_fallback_slow_subject_does_not_block_an_unrelated_subject() {
    let (source, gate) = fixture(Ok(false), true);
    let slow = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("slow-owner")).await })
    };
    source.entered.notified().await;
    source.paused.store(false, Ordering::SeqCst);
    assert!(!tokio::time::timeout(
        Duration::from_secs(1),
        gate.erasing(&SubjectId::new("other-owner"))
    )
    .await
    .unwrap()
    .unwrap());
    source.release.add_permits(1);
    assert!(!slow.await.unwrap().unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_fresh_snapshot_reconciles_a_cached_denial() {
    let (source, gate) = fixture(Ok(true), false);
    let owner = SubjectId::new("owner");
    assert!(gate.erasing(&owner).await.unwrap());
    *source.result.write().unwrap() = Ok(false);
    gate.refresh().await.unwrap();
    assert!(!gate.erasing(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_authoritative_checks_bypass_cached_denials() {
    let (source, gate) = fixture(Ok(true), false);
    let owner = SubjectId::new("owner");
    assert!(gate.erasing(&owner).await.unwrap());
    *source.result.write().unwrap() = Ok(false);
    assert!(!gate.authoritative(&owner).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stale_fallback_bounds_inflight_subjects_and_refuses_overflow_until_capacity_returns() {
    const CAPACITY: usize = 256;
    let (source, gate) = fixture(Ok(true), true);
    let barrier = Arc::new(Barrier::new(CAPACITY + 2));
    let mut checks = tokio::task::JoinSet::new();
    for index in 0..=CAPACITY {
        let barrier = barrier.clone();
        let gate = gate.clone();
        checks.spawn(async move {
            barrier.wait().await;
            gate.erasing(&SubjectId::new(format!("owner-{index}")))
                .await
        });
    }
    barrier.wait().await;
    source.entered.notified().await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while source.point_reads.load(Ordering::SeqCst) < CAPACITY {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(source.point_reads.load(Ordering::SeqCst), CAPACITY);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), checks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Err(AccountDeletionError::Unavailable)
    );
    source.release.add_permits(CAPACITY);
    while let Some(check) = checks.join_next().await {
        assert_eq!(check.unwrap(), Ok(true));
    }
    source.paused.store(false, Ordering::SeqCst);
    *source.result.write().unwrap() = Ok(false);
    assert!(!gate.erasing(&SubjectId::new("after-drain")).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), CAPACITY + 1);
}

#[tokio::test(start_paused = true)]
async fn stale_fallback_bounds_retained_denials_for_distinct_subjects() {
    let (source, gate) = fixture(Ok(true), false);
    for index in 0..257 {
        assert!(gate
            .erasing(&SubjectId::new(format!("owner-{index}")))
            .await
            .unwrap());
    }
    *source.result.write().unwrap() = Ok(false);
    let mut rechecked = 0;
    for index in 0..257 {
        if !gate
            .erasing(&SubjectId::new(format!("owner-{index}")))
            .await
            .unwrap()
        {
            rechecked += 1;
        }
    }
    assert!(
        rechecked > 0,
        "the bounded cache must evict completed denials"
    );
    assert!(source.point_reads.load(Ordering::SeqCst) > 257);
}

#[tokio::test]
async fn post_acceptance_request_cannot_join_an_older_allow_read() {
    let (source, gate) = fixture(Ok(false), true);
    let older = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
    };
    source.entered.notified().await;
    *source.result.write().unwrap() = Ok(true);
    source.paused.store(false, Ordering::SeqCst);
    let newer = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.erasing(&SubjectId::new("owner")).await })
    };
    tokio::task::yield_now().await;
    source.release.add_permits(1);
    assert!(newer.await.unwrap().unwrap());
    older.await.unwrap().unwrap();
}
