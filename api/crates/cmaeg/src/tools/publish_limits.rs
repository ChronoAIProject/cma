//! Conservative, record-scoped admission budgets for agent hand-offs.
use crate::AgentEventsConfig;
use cmaeg_core::{
    channel::AgentEventPolicy,
    ids::{ConsumerId, EventId, RecordId},
    seal::RootId,
};
use dashmap::DashMap;
use governor::{
    clock::{Clock, DefaultClock, Reference},
    state::{direct::NotKeyed, keyed::DashMapStateStore, InMemoryState},
    Quota, RateLimiter,
};
use std::{
    collections::BTreeSet,
    hash::Hash,
    num::NonZeroU32,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

const WINDOW: Duration = Duration::from_secs(60);
const ROOT_TTL: Duration = Duration::from_secs(1800);
const MAX_ROOT_EVENTS: usize = 16;
const SWEEP_EVERY: u64 = 256;
type Keyed<K, C> = RateLimiter<
    K,
    DashMapStateStore<K>,
    C,
    governor::middleware::NoOpMiddleware<<C as Clock>::Instant>,
>;
type Direct<C> = RateLimiter<
    NotKeyed,
    InMemoryState,
    C,
    governor::middleware::NoOpMiddleware<<C as Clock>::Instant>,
>;

struct Window<I> {
    start: I,
    used: u32,
}
struct RootBudget<I> {
    first_seen: I,
    published: BTreeSet<EventId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootLimit {
    EventCount,
    Capacity,
    Unavailable,
}

/// Fixed governor ceilings keep policy edits from resetting a limiter. The
/// per-record and per-consumer windows apply each current stored policy without
/// allocating one governor limiter per possible policy value.
pub struct PublishLimits<C: Clock = DefaultClock> {
    clock: C,
    per_consumer: Keyed<(RecordId, ConsumerId), C>,
    consumer_windows: DashMap<(RecordId, ConsumerId), Window<C::Instant>>,
    per_channel: Keyed<RecordId, C>,
    channel_windows: DashMap<RecordId, Window<C::Instant>>,
    per_owner: Keyed<String, C>,
    global: Direct<C>,
    roots: DashMap<(RecordId, String), RootBudget<C::Instant>>,
    root_capacity: usize,
    // Capacity, expiry and insertion are one transaction across map shards.
    root_admission: Mutex<()>,
    calls: AtomicU64,
}

impl PublishLimits {
    pub fn new(config: &AgentEventsConfig) -> Self {
        Self::with_clock(config, DefaultClock::default())
    }
}

impl<C: Clock + Clone> PublishLimits<C> {
    fn with_clock(config: &AgentEventsConfig, clock: C) -> Self {
        Self {
            per_consumer: keyed(30, clock.clone()),
            consumer_windows: DashMap::new(),
            per_channel: keyed((config.rate_per_minute / 10).max(1), clock.clone()),
            channel_windows: DashMap::new(),
            per_owner: keyed(config.owner_rate_per_minute, clock.clone()),
            global: RateLimiter::new(
                quota(config.rate_per_minute),
                InMemoryState::default(),
                clock.clone(),
            ),
            clock,
            roots: DashMap::new(),
            root_capacity: config.rate_per_minute as usize * 30,
            root_admission: Mutex::new(()),
            calls: AtomicU64::new(0),
        }
    }

    /// Narrower tiers are charged first. A later refusal retains earlier
    /// charges, so retries can only tighten admission, never weaken a budget.
    pub fn check_rates(
        &self,
        record: &RecordId,
        owner: &str,
        publisher: &ConsumerId,
        policy: &AgentEventPolicy,
    ) -> Result<(), Duration> {
        if self
            .calls
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .is_multiple_of(SWEEP_EVERY)
        {
            self.maintain();
        }
        let consumer = (record.clone(), publisher.clone());
        self.per_consumer
            .check_key(&consumer)
            .map_err(|until| until.wait_time_from(self.clock.now()))?;
        window(
            &self.consumer_windows,
            consumer,
            u32::from(policy.per_consumer_per_minute),
            self.clock.now(),
        )?;
        self.per_channel
            .check_key(record)
            .map_err(|until| until.wait_time_from(self.clock.now()))?;
        window(
            &self.channel_windows,
            record.clone(),
            u32::from(policy.per_channel_per_minute),
            self.clock.now(),
        )?;
        self.per_owner
            .check_key(&owner.to_owned())
            .map_err(|until| until.wait_time_from(self.clock.now()))?;
        self.global
            .check()
            .map_err(|until| until.wait_time_from(self.clock.now()))?;
        Ok(())
    }

    /// A retry keeps its first-seen expiry and succeeds even at the 16-ID bound.
    /// Live roots are never evicted: initial governor bursts can fill this map,
    /// so the configured global-rate × 30 capacity must fail closed.
    pub fn admit_root(
        &self,
        record: &RecordId,
        root: &RootId,
        event: &EventId,
    ) -> Result<(), RootLimit> {
        let _admission = self.root_admission.lock().map_err(|_| {
            tracing::error!("agent-event root admission lock poisoned; refusing publication");
            RootLimit::Unavailable
        })?;
        let now = self.clock.now();
        self.roots
            .retain(|_, budget| elapsed(now, budget.first_seen) < ROOT_TTL);
        let key = (record.clone(), root.as_str().to_owned());
        if let Some(mut budget) = self.roots.get_mut(&key) {
            if budget.published.contains(event) {
                return Ok(());
            }
            if budget.published.len() >= MAX_ROOT_EVENTS {
                return Err(RootLimit::EventCount);
            }
            budget.published.insert(event.clone());
            return Ok(());
        }
        if self.roots.len() >= self.root_capacity {
            return Err(RootLimit::Capacity);
        }
        self.roots.insert(
            key,
            RootBudget {
                first_seen: now,
                published: BTreeSet::from([event.clone()]),
            },
        );
        Ok(())
    }

    fn maintain(&self) {
        self.per_consumer.retain_recent();
        self.per_consumer.shrink_to_fit();
        self.per_channel.retain_recent();
        self.per_channel.shrink_to_fit();
        self.per_owner.retain_recent();
        self.per_owner.shrink_to_fit();
        let now = self.clock.now();
        self.consumer_windows
            .retain(|_, value| elapsed(now, value.start) < WINDOW);
        self.consumer_windows.shrink_to_fit();
        self.channel_windows
            .retain(|_, value| elapsed(now, value.start) < WINDOW);
        self.channel_windows.shrink_to_fit();
    }
}

fn elapsed<I: Reference>(now: I, start: I) -> Duration {
    now.duration_since(start).into()
}
fn quota(value: u32) -> Quota {
    Quota::per_minute(NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN))
}
fn keyed<K: Eq + Hash + Clone, C: Clock>(value: u32, clock: C) -> Keyed<K, C> {
    RateLimiter::new(quota(value), DashMapStateStore::default(), clock)
}
fn window<K: Eq + Hash, I: Reference>(
    windows: &DashMap<K, Window<I>>,
    key: K,
    bound: u32,
    now: I,
) -> Result<(), Duration> {
    let mut value = windows.entry(key).or_insert(Window {
        start: now,
        used: 0,
    });
    let age = elapsed(now, value.start);
    if age >= WINDOW {
        *value = Window {
            start: now,
            used: 0,
        };
    }
    if value.used >= bound {
        return Err(WINDOW.saturating_sub(elapsed(now, value.start)));
    }
    value.used += 1;
    Ok(())
}

#[cfg(test)]
#[path = "publish_limits_tests.rs"]
mod tests;
