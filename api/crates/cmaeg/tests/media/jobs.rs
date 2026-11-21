//! Single-flight jobs, the derived cache and per-channel fairness, under
//! paused time with fake jobs that count their executions.
use super::budget::{channel, limits};
use cmaeg::media::{
    budget::{ChannelLimiter, JobPermits},
    cache::{CacheKey, DerivedCache, Weighted},
    jobs::{JobAbort, JobMap, JobOutcome, MAX_LIVE_JOBS},
    MediaLimits,
};
use cmaeg_core::ids::ChannelId;
use futures_util::{future::BoxFuture, FutureExt};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Debug, PartialEq, Eq)]
struct Derived(Vec<u8>);

impl Weighted for Derived {
    fn weight(&self) -> u64 {
        self.0.len() as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    Abort(JobAbort),
    Job,
}

impl From<JobAbort> for Failure {
    fn from(abort: JobAbort) -> Self {
        Self::Abort(abort)
    }
}

type Map = JobMap<Derived, Failure>;

struct Jobs {
    map: Arc<Map>,
    permits: JobPermits,
}

fn jobs(cache_bytes: u64, max_jobs: u32) -> Jobs {
    let limits = MediaLimits {
        max_jobs,
        cache_bytes,
        ..limits()
    };
    let permits = JobPermits::new(&limits);
    let map = JobMap::new(
        DerivedCache::new(limits.cache_bytes),
        permits.clone(),
        Arc::new(ChannelLimiter::new(&limits)),
    );
    Jobs {
        map: Arc::new(map),
        permits,
    }
}

fn key(event: &str) -> CacheKey {
    CacheKey::new(
        &channel("jobs"),
        &event.parse().unwrap(),
        "activity-sha",
        0,
        "text",
    )
}

/// A job that counts its runs, takes `duration` and returns `bytes`.
fn counted(
    runs: &Arc<AtomicUsize>,
    duration: Duration,
    result: Result<usize, Failure>,
) -> impl FnOnce() -> BoxFuture<'static, Result<Derived, Failure>> + Send + 'static {
    let runs = runs.clone();
    move || {
        async move {
            runs.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(duration).await;
            result.map(|bytes| Derived(vec![7; bytes]))
        }
        .boxed()
    }
}

fn ready(outcome: JobOutcome<Derived, Failure>) -> Arc<Derived> {
    match outcome {
        JobOutcome::Ready(value) => value,
        other => panic!("expected a ready artifact, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn concurrent_calls_for_one_key_run_the_job_once() {
    let jobs = jobs(1 << 20, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    let a = channel("a");
    let wait = Duration::from_secs(10);
    let (first, second) = tokio::join!(
        jobs.map.run(
            key("e1"),
            &a,
            wait,
            counted(&runs, Duration::from_secs(1), Ok(4))
        ),
        jobs.map.run(
            key("e1"),
            &a,
            wait,
            counted(&runs, Duration::from_secs(1), Ok(4))
        ),
    );
    assert_eq!(ready(first), ready(second));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(!jobs.map.is_live(&key("e1")));
    assert_eq!(jobs.map.cached(&key("e1")).unwrap().0.len(), 4);
}

#[tokio::test(start_paused = true)]
async fn a_slow_job_is_pending_then_served_from_the_cache() {
    let jobs = jobs(1 << 20, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    let a = channel("a");
    let outcome = jobs
        .map
        .run(
            key("e1"),
            &a,
            Duration::from_secs(1),
            counted(&runs, Duration::from_secs(5), Ok(8)),
        )
        .await;
    assert!(matches!(
        outcome,
        JobOutcome::Pending {
            retry_after_ms: 1_000
        }
    ));
    assert!(jobs.map.is_live(&key("e1")));
    tokio::time::sleep(Duration::from_secs(10)).await;
    let value = ready(
        jobs.map
            .run(
                key("e1"),
                &a,
                Duration::from_secs(1),
                counted(&runs, Duration::ZERO, Ok(1)),
            )
            .await,
    );
    assert_eq!(value.0.len(), 8);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn failures_are_not_cached() {
    let jobs = jobs(1 << 20, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    let a = channel("a");
    let wait = Duration::from_secs(10);
    for _ in 0..2 {
        assert!(matches!(
            jobs.map
                .run(
                    key("e1"),
                    &a,
                    wait,
                    counted(&runs, Duration::ZERO, Err(Failure::Job))
                )
                .await,
            JobOutcome::Failed(Failure::Job)
        ));
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(jobs.map.cached(&key("e1")).is_none());
}

#[tokio::test(start_paused = true)]
async fn a_job_past_its_cap_is_timed_out() {
    let jobs = jobs(1 << 20, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    let outcome = jobs
        .map
        .run(
            key("e1"),
            &channel("a"),
            Duration::from_secs(200),
            counted(&runs, Duration::from_secs(100), Ok(1)),
        )
        .await;
    assert!(matches!(
        outcome,
        JobOutcome::Failed(Failure::Abort(JobAbort::TimedOut))
    ));
    assert!(!jobs.map.is_live(&key("e1")));
    assert!(jobs.map.cached(&key("e1")).is_none());
}

#[tokio::test(start_paused = true)]
async fn the_live_job_bound_answers_busy() {
    let jobs = jobs(1 << 20, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    for index in 0..MAX_LIVE_JOBS {
        let outcome = jobs
            .map
            .run(
                key(&format!("e{index}")),
                &channel(&format!("c{}", index % 64)),
                Duration::ZERO,
                counted(&runs, Duration::from_secs(60), Ok(1)),
            )
            .await;
        assert!(matches!(outcome, JobOutcome::Pending { .. }), "{index}");
    }
    let outcome = jobs
        .map
        .run(
            key("one-too-many"),
            &channel("c0"),
            Duration::ZERO,
            counted(&runs, Duration::ZERO, Ok(1)),
        )
        .await;
    assert!(matches!(
        outcome,
        JobOutcome::Failed(Failure::Abort(JobAbort::Busy))
    ));
    assert!(!jobs.map.is_live(&key("one-too-many")));
    // A live key is still joined at the bound.
    assert!(matches!(
        jobs.map
            .run(
                key("e0"),
                &channel("c0"),
                Duration::ZERO,
                counted(&runs, Duration::ZERO, Ok(1))
            )
            .await,
        JobOutcome::Pending { .. }
    ));
}

#[test]
fn weight_eviction_drops_the_oldest_entry() {
    let cache = DerivedCache::new(100);
    cache.insert(key("old"), Arc::new(Derived(vec![1; 60])));
    assert_eq!(cache.weighted_size(), 60);
    cache.insert(key("new"), Arc::new(Derived(vec![2; 60])));
    assert!(cache.get(&key("old")).is_none());
    assert_eq!(cache.get(&key("new")).unwrap().0, vec![2; 60]);
    assert_eq!(cache.weighted_size(), 60);
}

#[tokio::test(start_paused = true)]
async fn a_zero_cache_budget_never_caches() {
    let jobs = jobs(0, 6);
    let runs = Arc::new(AtomicUsize::new(0));
    let a = channel("a");
    for _ in 0..2 {
        ready(
            jobs.map
                .run(
                    key("e1"),
                    &a,
                    Duration::from_secs(10),
                    counted(&runs, Duration::ZERO, Ok(3)),
                )
                .await,
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(jobs.map.cached(&key("e1")).is_none());
}

/// Ten slow jobs on one channel, started without waiting, tracking how many
/// run at once.
fn flood(jobs: &Jobs, a: &ChannelId, running: &Arc<AtomicUsize>, peak: &Arc<AtomicUsize>) {
    for index in 0..10 {
        let (running, peak) = (running.clone(), peak.clone());
        let (map, a) = (jobs.map.clone(), a.clone());
        tokio::spawn(async move {
            let job = move || {
                async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(Derived(vec![1]))
                }
                .boxed()
            };
            map.run(key(&format!("a{index}")), &a, Duration::ZERO, job)
                .await;
        });
    }
}

#[tokio::test(start_paused = true)]
async fn one_channel_queue_does_not_delay_another_channel() {
    let jobs = jobs(1 << 20, 6);
    let (running, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let a = channel("a");
    flood(&jobs, &a, &running, &peak);
    tokio::time::sleep(Duration::from_millis(1)).await;
    let runs = Arc::new(AtomicUsize::new(0));
    let started = tokio::time::Instant::now();
    let value = ready(
        jobs.map
            .run(
                key("b"),
                &channel("b"),
                Duration::from_secs(5),
                counted(&runs, Duration::from_secs(1), Ok(2)),
            )
            .await,
    );
    assert_eq!(value.0.len(), 2);
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    // A still has queued jobs behind its two running ones.
    assert!(jobs.map.is_live(&key("a9")));
    assert_eq!(running.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn at_most_two_jobs_of_one_channel_run_at_once() {
    let jobs = jobs(1 << 20, 6);
    let (running, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let a = channel("a");
    flood(&jobs, &a, &running, &peak);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(running.load(Ordering::SeqCst), 0);
    for index in 0..10 {
        assert!(
            jobs.map.cached(&key(&format!("a{index}"))).is_some(),
            "{index}"
        );
    }
    assert_eq!(jobs.permits.available(), 6);
}

#[tokio::test(start_paused = true)]
async fn warm_does_nothing_while_permits_are_exhausted() {
    let jobs = jobs(1 << 20, 1);
    let runs = Arc::new(AtomicUsize::new(0));
    let busy = jobs
        .map
        .run(
            key("busy"),
            &channel("x"),
            Duration::ZERO,
            counted(&runs, Duration::from_secs(30), Ok(1)),
        )
        .await;
    assert!(matches!(busy, JobOutcome::Pending { .. }));
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(jobs.permits.available(), 0);
    let warmed = Arc::new(AtomicUsize::new(0));
    jobs.map.warm(
        key("warm"),
        &channel("y"),
        counted(&warmed, Duration::ZERO, Ok(1)),
    );
    assert!(!jobs.map.is_live(&key("warm")));
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(warmed.load(Ordering::SeqCst), 0);
    assert!(jobs.map.cached(&key("warm")).is_none());

    // With a permit free, warming runs the job into the cache.
    jobs.map.warm(
        key("warm"),
        &channel("y"),
        counted(&warmed, Duration::ZERO, Ok(1)),
    );
    assert!(jobs.map.is_live(&key("warm")));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(warmed.load(Ordering::SeqCst), 1);
    assert!(jobs.map.cached(&key("warm")).is_some());
    // A cached key is not warmed again.
    jobs.map.warm(
        key("warm"),
        &channel("y"),
        counted(&warmed, Duration::ZERO, Ok(1)),
    );
    assert!(!jobs.map.is_live(&key("warm")));
}

#[tokio::test(start_paused = true)]
async fn a_panicking_job_fails_its_waiters_and_leaves_the_map() {
    let jobs = jobs(1 << 20, 6);
    let outcome = jobs
        .map
        .run(key("panic"), &channel("a"), Duration::from_secs(10), || {
            async { panic!("job panicked") }.boxed()
        })
        .await;
    assert!(matches!(
        outcome,
        JobOutcome::Failed(Failure::Abort(JobAbort::Busy))
    ));
    assert!(!jobs.map.is_live(&key("panic")));
}
