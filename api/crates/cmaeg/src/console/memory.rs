//! The console's memory of channels: built from CMA's record heads and from
//! the observations the hot paths queue, and held only in this process.
//!
//! Nothing here is durable. A restart loses it and the index is listed again
//! from CMA; activity is relearned from traffic.
use std::{collections::BTreeSet, sync::Arc, time::Duration};

use moka::sync::Cache;

use super::{
    activity::{ActivityRings, Update},
    authority::Authorized,
    channel_ids::ChannelIds,
    cma_budget::{page_all, CmaBudget, Pacing, RecordHeads},
    index::{entry, IndexStatus, OwnerIndex},
    observer::{Observation, ObservationQueue},
    producers::Seen,
    reader::ChannelReader,
    summary::Summaries,
};
use crate::{http::readiness::Readiness, ConsoleMemoryConfig};

/// How long after `/readyz` first answers ready the index starts listing,
/// so it never competes with startup's channel restores.
pub const SETTLE: Duration = Duration::from_secs(30);
/// How often one owner's records may be re-listed on console access.
pub const RECONCILE_EVERY: Duration = Duration::from_secs(30);
/// Owners whose last re-listing is remembered.
const RECONCILED_OWNERS: u64 = 10_000;
/// How long a record that was not the reader's is not read again for them.
const MISS_TTL: Duration = Duration::from_secs(30);
/// Misses remembered.
const MISSES: u64 = 10_000;

/// Where the console reads CMA from.
pub struct Sources {
    /// Record heads, for the owner index.
    pub heads: Arc<dyn RecordHeads>,
    /// One record, for summaries and paste-links.
    pub reader: Arc<dyn ChannelReader>,
}

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

/// Everything the console remembers about channels. Its owner-checked
/// reads are in `reads.rs`.
pub struct ConsoleMemory {
    pub(super) index: OwnerIndex,
    pub(super) budget: CmaBudget,
    pub(super) summaries: Summaries,
    pub(super) channel_ids: ChannelIds,
    pub(super) rings: ActivityRings,
    pub(super) reader: Arc<dyn ChannelReader>,
    heads: Arc<dyn RecordHeads>,
    schedule: Schedule,
    /// Owners re-listed within the last [`RECONCILE_EVERY`].
    reconciled: Cache<String, ()>,
    /// `(owner, record)` reads that found no record of theirs, so probing
    /// unknown records cannot turn into a stream of CMA reads.
    pub(super) misses: Cache<(String, cmaeg_core::ids::RecordId), ()>,
    /// `CMAEG_NON_HUMAN_PROVIDERS`, read into each summary's consumers.
    pub(super) non_human_providers: BTreeSet<String>,
}

impl ConsoleMemory {
    pub fn new(config: &ConsoleMemoryConfig, sources: Sources, schedule: Schedule) -> Self {
        Self {
            index: OwnerIndex::new(config.index_max_records),
            budget: CmaBudget::default(),
            summaries: Summaries::new(config.summary_cache_max),
            channel_ids: ChannelIds::new(config.channel_ids_max),
            rings: ActivityRings::new(
                config.activity_per_channel,
                config.activity_max_bytes,
                config.producers_per_channel,
            ),
            reader: sources.reader,
            heads: sources.heads,
            schedule,
            reconciled: Cache::builder()
                .max_capacity(RECONCILED_OWNERS)
                .time_to_live(RECONCILE_EVERY)
                .build(),
            misses: Cache::builder()
                .max_capacity(MISSES)
                .time_to_live(MISS_TTL)
                .build(),
            non_human_providers: BTreeSet::new(),
        }
    }

    /// Read consumers' actor-kind capability against `providers`, the
    /// gateway's `CMAEG_NON_HUMAN_PROVIDERS`. Without it a consumer answers
    /// non-human actors only when it says so itself.
    pub fn with_non_human_providers(mut self, providers: BTreeSet<String>) -> Self {
        self.non_human_providers = providers;
        self
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
            Observation::ChannelId { record, id } => self.channel_ids.learn(record, id),
            Observation::Routed { record, entry_seed } => self.rings.admitted(record, entry_seed),
            Observation::Refused { record, seed } => self.rings.refused(record, seed),
            Observation::Launch {
                record,
                event_key,
                consumer,
                outcome,
                accepted_ms,
            } => self.rings.update(
                &record,
                &event_key,
                Update::Launch {
                    consumer,
                    outcome,
                    accepted_ms,
                },
            ),
            Observation::Delivery {
                record,
                event_key,
                consumer,
                step,
                outcome,
                destination,
            } => self.rings.update(
                &record,
                &event_key,
                Update::Delivery {
                    consumer,
                    step,
                    outcome,
                    destination,
                },
            ),
            Observation::Published {
                record,
                event_key,
                causation,
            } => self
                .rings
                .update(&record, &event_key, Update::Published(causation)),
            Observation::Producer {
                record,
                source_uri,
                hint,
                ce_type,
                outcome,
            } => self.rings.produced(
                record,
                Seen {
                    source_uri,
                    hint,
                    ce_type,
                    outcome,
                    at: time::OffsetDateTime::now_utc(),
                },
            ),
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
