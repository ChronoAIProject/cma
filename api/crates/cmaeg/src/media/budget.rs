//! Pod-wide byte and job budgets, and per-channel limits, for media work.
//!
//! The byte budgets count KiB so a 1 GiB budget fits a semaphore comfortably;
//! every reservation rounds up, so the budget is never exceeded. Each permit is
//! owned and releases its reservation when dropped, wherever it was moved.
use super::MediaLimits;
use crate::telemetry::names;
use cmaeg_core::ids::ChannelId;
use dashmap::DashMap;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{timeout_at, Instant},
};

const KIB: u64 = 1024;
/// Keyed state is swept for idle entries once every this many calls.
const SWEEP_EVERY: u64 = 256;
/// Download links a channel may mint per minute.
pub const LINK_MINTS_PER_MINUTE: u32 = 30;
/// One tenant must not hold the global job and decode permits.
pub const PER_CHANNEL_JOBS: usize = 2;

/// A byte budget counted in KiB, publishing its reserved bytes to a gauge.
#[derive(Clone)]
struct KibBudget {
    semaphore: Arc<Semaphore>,
    max_bytes: u64,
    total_kib: u64,
    gauge: &'static str,
}

impl KibBudget {
    fn new(max_bytes: u64, gauge: &'static str) -> Self {
        let total_kib = max_bytes.div_ceil(KIB);
        let permits = usize::try_from(total_kib)
            .unwrap_or(Semaphore::MAX_PERMITS)
            .min(Semaphore::MAX_PERMITS);
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
            max_bytes,
            total_kib,
            gauge,
        }
    }

    async fn acquire(&self, bytes: u64, deadline: Instant) -> Option<BytePermit> {
        let kib = u32::try_from(bytes.min(self.max_bytes).div_ceil(KIB)).ok()?;
        let permit = timeout_at(deadline, self.semaphore.clone().acquire_many_owned(kib))
            .await
            .ok()?
            .ok()?;
        self.publish();
        Some(BytePermit {
            permit: Some(permit),
            budget: self.clone(),
        })
    }

    fn available_bytes(&self) -> u64 {
        self.semaphore.available_permits() as u64 * KIB
    }

    fn publish(&self) {
        let reserved = self
            .total_kib
            .saturating_sub(self.semaphore.available_permits() as u64);
        metrics::gauge!(self.gauge).set((reserved * KIB) as f64);
    }
}

/// An owned reservation that updates its gauge once it is released.
struct BytePermit {
    permit: Option<OwnedSemaphorePermit>,
    budget: KibBudget,
}

impl Drop for BytePermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.budget.publish();
    }
}

/// Pod-wide bytes held by fetched originals and transform outputs.
#[derive(Clone)]
pub struct InflightBytes(KibBudget);

/// A reservation of [`InflightBytes`], released when dropped.
pub struct InflightPermit {
    _reservation: BytePermit,
}

impl InflightBytes {
    pub fn new(limits: &MediaLimits) -> Self {
        Self(KibBudget::new(
            limits.inflight_bytes,
            names::MEDIA_INFLIGHT_BYTES,
        ))
    }

    /// Reserve `min(bytes, budget)`, waiting until `deadline`. A request larger
    /// than the whole budget reserves all of it rather than failing, so one
    /// maximum-size original can always proceed alone.
    pub async fn acquire(&self, bytes: u64, deadline: Instant) -> Option<InflightPermit> {
        let reservation = self.0.acquire(bytes, deadline).await?;
        Some(InflightPermit {
            _reservation: reservation,
        })
    }

    pub fn available_bytes(&self) -> u64 {
        self.0.available_bytes()
    }
}

/// Pod-wide weighted bytes of media response bodies in flight.
#[derive(Clone)]
pub struct ResponseBudget(KibBudget);

/// A reservation of [`ResponseBudget`], released when dropped.
pub struct ResponsePermit {
    _reservation: BytePermit,
}

impl ResponseBudget {
    pub fn new(limits: &MediaLimits) -> Self {
        Self(KibBudget::new(
            limits.response_bytes,
            names::MEDIA_RESPONSE_BYTES,
        ))
    }

    /// Reserve `bytes`, waiting until `deadline`; a request larger than the
    /// whole budget can never be served and is refused at once.
    pub async fn acquire(&self, bytes: u64, deadline: Instant) -> Option<ResponsePermit> {
        if bytes > self.0.max_bytes {
            return None;
        }
        let reservation = self.0.acquire(bytes, deadline).await?;
        Some(ResponsePermit {
            _reservation: reservation,
        })
    }

    pub fn available_bytes(&self) -> u64 {
        self.0.available_bytes()
    }
}

/// Pod-wide concurrent media jobs.
#[derive(Clone)]
pub struct JobPermits {
    semaphore: Arc<Semaphore>,
    max_jobs: usize,
}

/// A held job slot; dropping it frees the slot and updates the gauge. A bare
/// semaphore permit has no release hook, so the gauge could not follow it.
pub struct JobPermit {
    permit: Option<OwnedSemaphorePermit>,
    jobs: JobPermits,
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.jobs.publish();
    }
}

impl JobPermits {
    pub fn new(limits: &MediaLimits) -> Self {
        let max_jobs = usize::try_from(limits.max_jobs).unwrap_or(usize::MAX);
        Self {
            semaphore: Arc::new(Semaphore::new(max_jobs.min(Semaphore::MAX_PERMITS))),
            max_jobs,
        }
    }

    pub async fn acquire(&self, deadline: Instant) -> Option<JobPermit> {
        let permit = timeout_at(deadline, self.semaphore.clone().acquire_owned())
            .await
            .ok()?
            .ok()?;
        self.publish();
        Some(JobPermit {
            permit: Some(permit),
            jobs: self.clone(),
        })
    }

    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    fn publish(&self) {
        let running = self.max_jobs.saturating_sub(self.available());
        metrics::gauge!(names::MEDIA_JOBS_INFLIGHT).set(running as f64);
    }
}

/// A per-channel GCRA quota whose idle keys are swept, so a flood of distinct
/// channels cannot grow it without bound.
pub struct KeyedQuota {
    limiter: DefaultKeyedRateLimiter<ChannelId>,
    checks: AtomicU64,
}

impl KeyedQuota {
    /// `n` cells per minute, with a burst of `n`.
    pub fn per_minute(n: NonZeroU32) -> Self {
        Self::new(Quota::per_minute(n))
    }

    /// `n` cells per hour, with a burst of `n`.
    pub fn per_hour(n: NonZeroU32) -> Self {
        Self::new(Quota::per_hour(n))
    }

    fn new(quota: Quota) -> Self {
        Self {
            limiter: RateLimiter::dashmap(quota),
            checks: AtomicU64::new(0),
        }
    }

    pub fn check(&self, channel: &ChannelId) -> bool {
        self.check_n(channel, 1)
    }

    /// Take `n` cells at once, or none. A batch larger than the burst can never
    /// conform and is refused like one that must wait.
    pub fn check_n(&self, channel: &ChannelId, n: u32) -> bool {
        if self
            .checks
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY)
        {
            self.limiter.retain_recent();
            self.limiter.shrink_to_fit();
        }
        let Some(n) = NonZeroU32::new(n) else {
            return true;
        };
        matches!(self.limiter.check_key_n(channel, n), Ok(Ok(())))
    }

    /// Keys currently retained; idle keys leave at the next sweep.
    pub fn len(&self) -> usize {
        self.limiter.len()
    }

    pub fn is_empty(&self) -> bool {
        self.limiter.is_empty()
    }
}

/// Per-channel rate, byte and job limits.
pub struct ChannelLimiter {
    /// Fetch starts per minute.
    pub fetch: KeyedQuota,
    /// Download-link mints per minute.
    pub link: KeyedQuota,
    /// Download KiB per hour.
    pub download_kib: KeyedQuota,
    /// At most [`PER_CHANNEL_JOBS`] derivations per channel at once.
    pub slots: DashMap<ChannelId, Arc<Semaphore>>,
    slot_calls: AtomicU64,
}

impl ChannelLimiter {
    pub fn new(limits: &MediaLimits) -> Self {
        let download_kib = u32::try_from(limits.download_bytes_per_hour / KIB).unwrap_or(u32::MAX);
        Self {
            fetch: KeyedQuota::per_minute(at_least_one(limits.fetch_per_minute)),
            link: KeyedQuota::per_minute(at_least_one(LINK_MINTS_PER_MINUTE)),
            download_kib: KeyedQuota::per_hour(at_least_one(download_kib)),
            slots: DashMap::new(),
            slot_calls: AtomicU64::new(0),
        }
    }

    /// Wait, first come first served, for one of the channel's job slots.
    pub async fn derive_slot(
        &self,
        channel: &ChannelId,
        deadline: Instant,
    ) -> Option<OwnedSemaphorePermit> {
        if self
            .slot_calls
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY)
        {
            // `retain` holds each shard's write lock while it decides, so no
            // clone can be taken from an entry while it is being removed; an
            // entry is idle when nothing but the map holds it and every slot
            // is free.
            self.slots.retain(|_, semaphore| {
                !(Arc::strong_count(semaphore) == 1
                    && semaphore.available_permits() == PER_CHANNEL_JOBS)
            });
        }
        let semaphore = self
            .slots
            .entry(channel.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(PER_CHANNEL_JOBS)))
            .clone();
        timeout_at(deadline, semaphore.acquire_owned())
            .await
            .ok()?
            .ok()
    }
}

/// Configuration bounds every rate at one or more; a hand-built zero would
/// otherwise make a quota that can never admit anything.
pub(in crate::media) fn at_least_one(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}
