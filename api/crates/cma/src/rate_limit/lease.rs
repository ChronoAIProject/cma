//! Bounded, prepaid Default-tier credits. Mongo remains the budget authority.

use super::shared::Refusal;
use layer2_core::StoreError;
use std::{collections::HashMap, sync::Arc, sync::Mutex, time::Duration};
use tokio::{sync::Mutex as AsyncMutex, time::Instant};

const LEASE_DURATION: Duration = Duration::from_secs(10);
const MAX_KEYS: usize = 16_384;

#[async_trait::async_trait]
pub(super) trait Charge: Send + Sync {
    async fn charge(&self, key: &str, limit: u32, tokens: u32) -> Result<Option<u64>, StoreError>;
}

#[async_trait::async_trait]
impl Charge for crate::store::request_limits::MongoRequestLimits {
    async fn charge(&self, key: &str, limit: u32, tokens: u32) -> Result<Option<u64>, StoreError> {
        self.charge(key, limit, tokens).await
    }
}

struct Lease {
    remaining: u32,
    expires: Instant,
    window_started: Instant,
    arrivals: u32,
    previous_arrivals: u32,
}

impl Lease {
    fn new(now: Instant) -> Self {
        Self {
            remaining: 0,
            expires: now + LEASE_DURATION,
            window_started: now,
            arrivals: 0,
            previous_arrivals: 0,
        }
    }

    fn observe(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.window_started);
        if elapsed >= LEASE_DURATION {
            self.previous_arrivals = if elapsed < LEASE_DURATION * 2 {
                self.arrivals
            } else {
                0
            };
            self.arrivals = 0;
            self.window_started = now;
        }
        self.arrivals = self.arrivals.saturating_add(1);
        if now >= self.expires {
            self.discard();
        }
    }

    fn discard(&mut self) {
        metrics::counter!("cma_rate_default_lease_discarded_total")
            .increment(u64::from(self.remaining));
        self.remaining = 0;
    }

    fn size(&self, limit: u32) -> u32 {
        self.previous_arrivals.clamp(1, (limit / 20).max(1))
    }
}

struct Table {
    entries: HashMap<String, Arc<AsyncMutex<Lease>>>,
    next_cleanup: Instant,
}

pub(super) struct LeaseTable {
    table: Mutex<Table>,
    capacity: usize,
}

impl Default for LeaseTable {
    fn default() -> Self {
        Self::new(MAX_KEYS)
    }
}

impl LeaseTable {
    fn new(capacity: usize) -> Self {
        Self {
            table: Mutex::new(Table {
                entries: HashMap::new(),
                next_cleanup: Instant::now(),
            }),
            capacity,
        }
    }

    fn entry(&self, key: &str) -> Option<Arc<AsyncMutex<Lease>>> {
        let now = Instant::now();
        let mut table = self.table.lock().expect("lease table lock poisoned");
        if let Some(entry) = table.entries.get(key) {
            return Some(entry.clone());
        }
        // Reclaim idle expired keys on insertion, at most once a second.
        // A caller's Arc also protects entries awaiting their per-key lock:
        // eviction must never create a second concurrent charge for that key.
        if now >= table.next_cleanup {
            table.entries.retain(|_, entry| {
                if Arc::strong_count(entry) == 1 {
                    if let Ok(mut lease) = entry.try_lock() {
                        if now >= lease.expires {
                            lease.discard();
                            return false;
                        }
                    }
                }
                true
            });
            table.next_cleanup = now + Duration::from_secs(1);
        }
        if table.entries.len() >= self.capacity {
            return None;
        }
        let entry = Arc::new(AsyncMutex::new(Lease::new(now)));
        table.entries.insert(key.to_owned(), entry.clone());
        Some(entry)
    }

    pub(super) async fn check(
        &self,
        store: &impl Charge,
        key: &str,
        limit: u32,
    ) -> Result<(), Refusal> {
        let outcome = self.admit(store, key, limit).await;
        record(match &outcome {
            Ok(source) => source,
            Err(Refusal::Limited(_)) => "refused",
            Err(Refusal::Unavailable) => "unavailable",
        });
        outcome.map(|_| ())
    }

    async fn admit(
        &self,
        store: &impl Charge,
        key: &str,
        limit: u32,
    ) -> Result<&'static str, Refusal> {
        let Some(entry) = self.entry(key) else {
            charge(store, key, limit, 1).await?;
            return Ok("charged");
        };
        let mut lease = entry.lock().await;
        let started = Instant::now();
        lease.observe(started);
        if lease.remaining > 0 {
            lease.remaining -= 1;
            return Ok("leased");
        }
        let mut tokens = lease.size(limit);
        match charge(store, key, limit, tokens).await {
            Err(Refusal::Limited(_)) if tokens > 1 => {
                // Preserve the exact single-request result and Retry-After
                // near the boundary instead of refusing a whole batch.
                tokens = 1;
                charge(store, key, limit, tokens).await?;
            }
            result => result?,
        }
        lease.remaining = tokens - 1;
        // Include Mongo wait in the lifetime. Cancelling an ambiguous charge
        // can waste prepayment, but it must never mint local credits.
        lease.expires = started + LEASE_DURATION;
        Ok("charged")
    }
}

async fn charge(store: &impl Charge, key: &str, limit: u32, tokens: u32) -> Result<(), Refusal> {
    match store.charge(key, limit, tokens).await {
        Ok(None) => Ok(()),
        Ok(Some(delay)) => Err(Refusal::Limited(delay)),
        Err(_) => {
            tracing::warn!("shared request budget unavailable");
            Err(Refusal::Unavailable)
        }
    }
}

pub(super) fn record(source: &'static str) {
    metrics::counter!("cma_rate_default_decisions_total", "source" => source).increment(1);
}

#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;
