//! The console's memory of channels: built from CMA's record heads and from
//! the observations the hot paths queue, and held only in this process.
//!
//! Nothing here is durable. A restart loses it and the index is listed again
//! from CMA; activity is relearned from traffic.
use std::{sync::Arc, time::Duration};

use moka::sync::Cache;

use super::{
    authority::Authorized,
    cma_budget::{page_all, CmaBudget, Pacing, RecordHeads},
    index::{entry, IndexStatus, OwnerIndex},
    observer::{Observation, ObservationQueue},
};
use crate::{http::readiness::Readiness, ConsoleMemoryConfig};

/// How long after `/readyz` first answers ready the index starts listing,
/// so it never competes with startup's channel restores.
pub const SETTLE: Duration = Duration::from_secs(30);
/// How often one owner's records may be re-listed on console access.
pub const RECONCILE_EVERY: Duration = Duration::from_secs(30);
/// Owners whose last re-listing is remembered.
const RECONCILED_OWNERS: u64 = 10_000;

/// When the index lists CMA.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    /// The pause after readiness before the first listing.
    pub settle: Duration,
    /// The pause between full listings.
    pub resync: Duration,
    pub pacing: Pacing,
}

impl Schedule {
    pub fn from_config(config: &ConsoleMemoryConfig) -> Self {
        Self {
            settle: SETTLE,
            resync: config.index_resync,
            pacing: Pacing::default(),
        }
    }
}

/// Everything the console remembers about channels.
pub struct ConsoleMemory {
    index: OwnerIndex,
    budget: CmaBudget,
    heads: Arc<dyn RecordHeads>,
    schedule: Schedule,
    /// Owners re-listed within the last [`RECONCILE_EVERY`].
    reconciled: Cache<String, ()>,
}

impl ConsoleMemory {
    pub fn new(
        config: &ConsoleMemoryConfig,
        heads: Arc<dyn RecordHeads>,
        schedule: Schedule,
    ) -> Self {
        Self {
            index: OwnerIndex::new(config.index_max_records),
            budget: CmaBudget::default(),
            heads,
            schedule,
            reconciled: Cache::builder()
                .max_capacity(RECONCILED_OWNERS)
                .time_to_live(RECONCILE_EVERY)
                .build(),
        }
    }

    /// Apply one observation from the queue.
    pub fn apply(&self, observation: Observation) {
        match observation {
            Observation::IndexChanged {
                record,
                owner,
                version,
            } => self.index.upsert(record, owner, version),
            Observation::IndexRemoved { record } => self.index.remove(&record),
            // Kept by the structures added alongside the index.
            Observation::Routed { .. }
            | Observation::Refused { .. }
            | Observation::Launch { .. }
            | Observation::Delivery { .. }
            | Observation::Published { .. }
            | Observation::Producer { .. }
            | Observation::ChannelId { .. } => {}
        }
    }

    pub fn index_status(&self) -> IndexStatus {
        self.index.status()
    }

    /// List every record head from CMA, paced, and drop records it no
    /// longer has. A failed listing removes nothing.
    pub async fn list_all(&self) {
        let since = self.index.begin(true);
        let mut seen = 0u64;
        let listed = page_all(
            self.heads.as_ref(),
            &self.budget,
            self.schedule.pacing,
            None,
            |page| {
                for (record, owner, version) in page.iter().filter_map(entry) {
                    self.index.upsert(record, owner, version);
                }
                seen += page.len() as u64;
                self.index.progress(seen);
            },
        )
        .await;
        match listed {
            Ok(count) => {
                self.index.retire(since, None);
                tracing::info!(listed = count, "console index listed");
            }
            Err(error) => {
                self.index.abandon();
                tracing::warn!(%error, listed = seen, "console index listing failed");
            }
        }
    }

    /// Re-list `auth`'s records from CMA, at most once per
    /// [`RECONCILE_EVERY`] per owner; true when it listed.
    pub async fn reconcile(&self, auth: &Authorized) -> bool {
        let fresh = self
            .reconciled
            .entry(auth.owner().to_owned())
            .or_insert(())
            .is_fresh();
        if !fresh {
            return false;
        }
        let since = self.index.begin(false);
        let listed = page_all(
            self.heads.as_ref(),
            &self.budget,
            self.schedule.pacing,
            Some(auth.owner()),
            |page| {
                for (record, owner, version) in page.iter().filter_map(entry) {
                    self.index.upsert(record, owner, version);
                }
            },
        )
        .await;
        match listed {
            Ok(_) => self.index.retire(since, Some(auth)),
            Err(error) => {
                // Let the next access try again.
                self.reconciled.invalidate(auth.owner());
                tracing::warn!(%error, "console owner reconcile failed");
            }
        }
        true
    }

    /// Start the console's background work: the one drainer of `queue`, and
    /// the index's listings, the first [`Schedule::settle`] after `/readyz`
    /// first answers ready and then every resync interval. Detached: both
    /// run for the life of the process.
    pub fn spawn(self: &Arc<Self>, queue: ObservationQueue, readiness: Readiness) {
        let memory = self.clone();
        drop(tokio::spawn(async move {
            queue.drain(|observation| memory.apply(observation)).await;
        }));
        let memory = self.clone();
        drop(tokio::spawn(async move {
            readiness.served().await;
            tokio::time::sleep(memory.schedule.settle).await;
            loop {
                memory.list_all().await;
                tokio::time::sleep(memory.schedule.resync).await;
            }
        }));
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
