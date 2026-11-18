//! Bounded advisory retry timing. Entries never confer durable authority.
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::time::Instant;

pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

pub(crate) trait Jitter: Send + Sync {
    fn extra(&self, maximum: Duration) -> Duration;
}

pub(crate) struct MonotonicClock;
impl Clock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub(crate) struct RandomJitter;
impl Jitter for RandomJitter {
    fn extra(&self, maximum: Duration) -> Duration {
        let random = uuid::Uuid::new_v4().as_u128() as u64;
        Duration::from_millis(random % (maximum.as_millis() as u64 + 1))
    }
}

pub(crate) struct RetryPolicy {
    pub initial: Duration,
    pub maximum: Duration,
    pub retention: Duration,
}

struct Entry {
    failures: u32,
    next: Instant,
    observed: Instant,
}

pub(crate) struct RetrySchedule<K> {
    capacity: usize,
    entries: BTreeMap<K, Entry>,
    policy: RetryPolicy,
    clock: Arc<dyn Clock>,
    jitter: Arc<dyn Jitter>,
}

impl<K: Ord + Clone> RetrySchedule<K> {
    pub(crate) fn new(
        capacity: usize,
        policy: RetryPolicy,
        clock: Arc<dyn Clock>,
        jitter: Arc<dyn Jitter>,
    ) -> Self {
        Self {
            capacity,
            entries: BTreeMap::new(),
            policy,
            clock,
            jitter,
        }
    }

    pub(crate) fn prune(&mut self) {
        let now = self.clock.now();
        self.entries.retain(|_, entry| {
            now.saturating_duration_since(entry.observed) < self.policy.retention
        });
    }

    pub(crate) fn remaining(&mut self, key: &K) -> Duration {
        self.prune();
        self.entries.get(key).map_or(Duration::ZERO, |entry| {
            entry.next.saturating_duration_since(self.clock.now())
        })
    }

    /// Inspect the retained set after admission/failure has pruned it once.
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn recovered(&mut self, key: &K) {
        self.entries.remove(key);
    }

    pub(crate) fn failed(&mut self, key: K) -> (u32, Duration) {
        self.prune();
        if self.capacity == 0 {
            return (0, Duration::ZERO);
        }
        let failures = self
            .entries
            .get(&key)
            .map_or(1, |entry| entry.failures.saturating_add(1));
        let multiplier = 1_u32 << failures.saturating_sub(1).min(31);
        let base = self
            .policy
            .initial
            .saturating_mul(multiplier)
            .min(self.policy.maximum);
        let allowance = (base / 10).min(Duration::from_secs(1));
        let delay = base
            .saturating_add(self.jitter.extra(allowance).min(allowance))
            .min(self.policy.maximum);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            // Eviction only permits an extra attempt. Custody lives elsewhere.
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.observed)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let now = self.clock.now();
        self.entries.insert(
            key,
            Entry {
                failures,
                next: now + delay,
                observed: now,
            },
        );
        (failures, delay)
    }
}

#[cfg(test)]
mod tests;
