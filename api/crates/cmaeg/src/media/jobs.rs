//! Single-flight media jobs in front of the derived cache.
//!
//! Concurrent requests for one artifact share one job. A job first waits for
//! one of its channel's slots, then for a pod-wide permit, so a channel with a
//! queue never holds global permits while it waits. A caller waits a bounded
//! time and is told to retry while the job finishes into the cache in the
//! background; failures are never cached.
use super::{
    budget::{ChannelLimiter, JobPermits},
    cache::{CacheKey, DerivedCache, Weighted},
};
use cmaeg_core::ids::ChannelId;
use dashmap::{mapref::entry::Entry, DashMap};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    time::{timeout, timeout_at, Instant},
};

/// Each waiting job is a small task; the bound keeps a flood of distinct keys
/// from growing memory. The per-channel fetch quota normally binds first.
pub const MAX_LIVE_JOBS: usize = 1024;
/// The background budget for one job, from spawn to its result.
pub const JOB_CAP: Duration = Duration::from_secs(90);
const RETRY_AFTER_MS: u64 = 1_000;

/// What a caller gets back within its wait.
#[derive(Debug)]
pub enum JobOutcome<T, E> {
    Ready(Arc<T>),
    /// Still running; it will land in the cache if it succeeds.
    Pending {
        retry_after_ms: u64,
    },
    Failed(E),
}

/// Why the map, rather than the job, ended a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobAbort {
    /// The job did not finish within [`JOB_CAP`].
    TimedOut,
    /// Too many live jobs, or a job ended without a result.
    Busy,
}

type Published<T, E> = Option<Result<Arc<T>, E>>;

/// Live jobs by key, the cache they fill and the permits they run under.
pub struct JobMap<T: Weighted + Send + Sync + 'static, E> {
    live: Arc<DashMap<CacheKey, watch::Receiver<Published<T, E>>>>,
    cache: DerivedCache<T>,
    permits: JobPermits,
    limiter: Arc<ChannelLimiter>,
}

impl<T, E> JobMap<T, E>
where
    T: Weighted + Send + Sync + 'static,
    E: Clone + Send + Sync + From<JobAbort> + 'static,
{
    pub fn new(cache: DerivedCache<T>, permits: JobPermits, limiter: Arc<ChannelLimiter>) -> Self {
        Self {
            live: Arc::new(DashMap::new()),
            cache,
            permits,
            limiter,
        }
    }

    /// The cached artifact, without starting anything.
    pub fn cached(&self, key: &CacheKey) -> Option<Arc<T>> {
        self.cache.get(key)
    }

    /// Whether a job for `key` is running or queued. With [`Self::cached`]
    /// this lets callers charge a per-channel quota only when a new job would
    /// start.
    pub fn is_live(&self, key: &CacheKey) -> bool {
        self.live.contains_key(key)
    }

    /// The artifact for `key`: from the cache, from a running job, or from a
    /// new job, waiting at most `wait` before answering `Pending`.
    pub async fn run(
        &self,
        key: CacheKey,
        channel: &ChannelId,
        wait: Duration,
        job: impl FnOnce() -> BoxFuture<'static, Result<T, E>> + Send + 'static,
    ) -> JobOutcome<T, E> {
        if let Some(value) = self.cache.get(&key) {
            return JobOutcome::Ready(value);
        }
        match self.join_or_start(key, channel, job) {
            Ok(receiver) => wait_for(receiver, wait).await,
            Err(outcome) => outcome,
        }
    }

    /// Start `job` in the background when a pod-wide permit is free and the
    /// artifact is neither cached nor already being made.
    pub fn warm(
        &self,
        key: CacheKey,
        channel: &ChannelId,
        job: impl FnOnce() -> BoxFuture<'static, Result<T, E>> + Send + 'static,
    ) {
        if self.permits.available() == 0 || self.cache.get(&key).is_some() || self.is_live(&key) {
            return;
        }
        if let Err(JobOutcome::Failed(_)) = self.join_or_start(key, channel, job) {
            tracing::debug!(?key, "media warm-up skipped: too many live jobs");
        }
    }

    /// Subscribe to the live job for `key`, or start one. `Err` carries an
    /// immediate answer: a value cached meanwhile, or `Busy` at the bound.
    fn join_or_start(
        &self,
        key: CacheKey,
        channel: &ChannelId,
        job: impl FnOnce() -> BoxFuture<'static, Result<T, E>> + Send + 'static,
    ) -> Result<watch::Receiver<Published<T, E>>, JobOutcome<T, E>> {
        if let Some(receiver) = self.live.get(&key) {
            return Ok(receiver.clone());
        }
        // Counted before taking an entry: `len` locks every shard.
        if self.live.len() >= MAX_LIVE_JOBS {
            tracing::info!(
                live = MAX_LIVE_JOBS,
                "media job refused: too many live jobs"
            );
            return Err(JobOutcome::Failed(JobAbort::Busy.into()));
        }
        match self.live.entry(key) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                // A job that finished since the caller's cache check cached
                // its value before leaving the live map.
                if let Some(value) = self.cache.get(&key) {
                    return Err(JobOutcome::Ready(value));
                }
                let (sender, receiver) = watch::channel(None);
                entry.insert(receiver.clone());
                self.spawn(key, channel.clone(), sender, receiver.clone(), job);
                Ok(receiver)
            }
        }
    }

    fn spawn(
        &self,
        key: CacheKey,
        channel: ChannelId,
        sender: watch::Sender<Published<T, E>>,
        receiver: watch::Receiver<Published<T, E>>,
        job: impl FnOnce() -> BoxFuture<'static, Result<T, E>> + Send + 'static,
    ) {
        // Removes the live entry however the task ends, a panic included.
        let entry = LiveEntry {
            live: self.live.clone(),
            key,
            receiver,
        };
        let cache = self.cache.clone();
        let permits = self.permits.clone();
        let limiter = self.limiter.clone();
        tokio::spawn(async move {
            let _entry = entry;
            let deadline = Instant::now() + JOB_CAP;
            let run = async {
                // The channel's slot comes first, FIFO, so a channel with a
                // queue never holds pod-wide permits while it waits.
                let _slot = limiter
                    .derive_slot(&channel, deadline)
                    .await
                    .ok_or(JobAbort::TimedOut)?;
                let _permit = permits.acquire(deadline).await.ok_or(JobAbort::TimedOut)?;
                Ok::<_, JobAbort>(job().await)
            };
            let result = match timeout_at(deadline, run).await {
                Ok(Ok(Ok(value))) => {
                    let value = Arc::new(value);
                    cache.insert(key, value.clone());
                    Ok(value)
                }
                Ok(Ok(Err(error))) => Err(error),
                Ok(Err(abort)) => Err(abort.into()),
                Err(_) => Err(JobAbort::TimedOut.into()),
            };
            if result.is_err() {
                tracing::debug!(?key, "media job failed; the failure is not cached");
            }
            sender.send_replace(Some(result));
        });
    }
}

/// Wait up to `wait` for the job's published result.
async fn wait_for<T, E>(
    mut receiver: watch::Receiver<Published<T, E>>,
    wait: Duration,
) -> JobOutcome<T, E>
where
    E: Clone + From<JobAbort>,
{
    let published = timeout(wait, async {
        loop {
            let current = receiver.borrow_and_update().clone();
            if current.is_some() {
                return current;
            }
            if receiver.changed().await.is_err() {
                // The job ended; it may still have published on its way out.
                return receiver.borrow().clone();
            }
        }
    })
    .await;
    match published {
        Err(_) => JobOutcome::Pending {
            retry_after_ms: RETRY_AFTER_MS,
        },
        Ok(Some(Ok(value))) => JobOutcome::Ready(value),
        Ok(Some(Err(error))) => JobOutcome::Failed(error),
        // The job's task ended without a result, for example by panicking.
        Ok(None) => JobOutcome::Failed(JobAbort::Busy.into()),
    }
}

/// The live-map entry of one job task.
struct LiveEntry<T, E> {
    live: Arc<DashMap<CacheKey, watch::Receiver<Published<T, E>>>>,
    key: CacheKey,
    receiver: watch::Receiver<Published<T, E>>,
}

impl<T, E> Drop for LiveEntry<T, E> {
    fn drop(&mut self) {
        self.live
            .remove_if(&self.key, |_, current| current.same_channel(&self.receiver));
    }
}
