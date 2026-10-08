//! Non-waiting leases for routed turns sharing the existing provider pools.
//! A record, rather than a sealed channel-id string, is the capacity identity.
use crate::{telemetry, RoutedCapacityConfig};
use cmaeg_core::ids::RecordId;
use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct RoutedCapacity {
    per_channel: u32,
    per_owner: u32,
    channels: DashMap<RecordId, Arc<Semaphore>>,
    owners: DashMap<String, Arc<Semaphore>>,
    attempts: AtomicU64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityScope {
    Channel,
    Owner,
}
impl CapacityScope {
    pub fn code(self) -> &'static str {
        match self {
            Self::Channel => "channel_capacity",
            Self::Owner => "owner_capacity",
        }
    }
}
impl RoutedCapacity {
    pub fn new(config: RoutedCapacityConfig) -> Self {
        Self {
            per_channel: config.per_channel,
            per_owner: config.per_owner,
            channels: DashMap::new(),
            owners: DashMap::new(),
            attempts: AtomicU64::new(0),
        }
    }
    /// Acquire both permits or neither, without waiting. Count failed attempts
    /// toward cleanup too: an exhausted owner must not accumulate idle records.
    pub fn try_lease(&self, record: &RecordId, owner: &str) -> Result<TurnLease, CapacityScope> {
        if self
            .attempts
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .is_multiple_of(256)
        {
            self.sweep();
        }
        // Clone under the entry lock. The sweep retains every external Arc,
        // including one whose caller has not acquired a permit yet.
        let channel = {
            let entry = self
                .channels
                .entry(record.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_channel as usize)));
            Arc::clone(entry.value())
        };
        let channel = channel
            .try_acquire_owned()
            .map_err(|_| self.refused(record, CapacityScope::Channel))?;
        // Never hold guards from both maps at once: sweeping also walks them.
        let owner = {
            let entry = self
                .owners
                .entry(owner.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_owner as usize)));
            Arc::clone(entry.value())
        };
        let owner = match owner.try_acquire_owned() {
            Ok(owner) => owner,
            Err(_) => {
                drop(channel);
                return Err(self.refused(record, CapacityScope::Owner));
            }
        };
        metrics::gauge!(telemetry::names::ROUTED_TURNS_IN_FLIGHT).increment(1);
        Ok(TurnLease {
            _channel: channel,
            _owner: owner,
        })
    }
    fn refused(&self, record: &RecordId, scope: CapacityScope) -> CapacityScope {
        let in_flight = match scope {
            CapacityScope::Channel => self.per_channel,
            CapacityScope::Owner => self.per_owner,
        };
        tracing::warn!(record_hash = %telemetry::channel_hash(record.as_str()),
            scope = scope.code(), in_flight, "routed turn capacity refused");
        scope
    }
    fn sweep(&self) {
        // Active/acquiring entries plus idle entries since the last sweep. Tracker admission happens after leases are acquired,
        // so its 4,096 tasks are not an exact bound on these maps.
        self.channels.retain(|_, semaphore| {
            semaphore.available_permits() != self.per_channel as usize
                || Arc::strong_count(semaphore) > 1
        });
        self.owners.retain(|_, semaphore| {
            semaphore.available_permits() != self.per_owner as usize
                || Arc::strong_count(semaphore) > 1
        });
    }
}

/// Owns capacity only until the runner exits. Delivery may retain the turn
/// request after that, so the runner takes this value out of the request.
#[derive(Debug)]
pub struct TurnLease {
    _channel: OwnedSemaphorePermit,
    _owner: OwnedSemaphorePermit,
}
impl Drop for TurnLease {
    fn drop(&mut self) {
        metrics::gauge!(telemetry::names::ROUTED_TURNS_IN_FLIGHT).decrement(1);
    }
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
