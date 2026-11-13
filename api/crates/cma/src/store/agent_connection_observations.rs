//! Reuse pure candidate validation only for an identical fresh complete BSON row.
use std::{collections::HashMap, sync::Mutex, time::Duration};

use layer2_core::sandbox::store::{AgentConnectionRecoveryCandidate, SandboxStoreError};
use mongodb::bson::RawDocument;
use tokio::time::Instant;

const MAX_ENTRIES: usize = 1024;
const MAX_ROW_BYTES: usize = 256 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(30);

struct Entry {
    row: Box<[u8]>,
    candidate: AgentConnectionRecoveryCandidate,
    expires: Instant,
    charge: usize,
}

struct State {
    entries: HashMap<String, Entry>,
    bytes: usize,
}

// Reserve the maximum map once. Charge four bucket footprints per supported
// entry plus allocation slack for owned strings; the payload budget includes
// this conservative metadata reservation even when the map is empty.
const METADATA_BYTES: usize =
    MAX_ENTRIES * (4 * std::mem::size_of::<(String, Entry)>() + 128) + std::mem::size_of::<State>();
const _: () = assert!(METADATA_BYTES < MAX_BYTES);

impl Default for State {
    fn default() -> Self {
        Self {
            entries: HashMap::with_capacity(MAX_ENTRIES),
            bytes: METADATA_BYTES,
        }
    }
}

impl State {
    fn remove(&mut self, id: &str) {
        if let Some(entry) = self.entries.remove(id) {
            self.bytes -= entry.charge;
            metrics::gauge!("cma_agent_discovery_cache_bytes").set(self.bytes as f64);
            metrics::gauge!("cma_agent_discovery_cache_entries").set(self.entries.len() as f64);
        }
    }

    fn expire(&mut self, now: Instant) {
        self.entries.retain(|_, entry| {
            if now >= entry.expires {
                self.bytes -= entry.charge;
                false
            } else {
                true
            }
        });
    }
}

#[derive(Default)]
pub(super) struct Observations {
    state: Mutex<State>,
    #[cfg(test)]
    pub(super) hits: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    pub(super) validations: std::sync::atomic::AtomicU64,
}

impl Observations {
    pub(super) fn pass(&self) -> Pass<'_> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        metrics::gauge!("cma_agent_discovery_cache_bytes").set(state.bytes as f64);
        metrics::gauge!("cma_agent_discovery_cache_entries").set(state.entries.len() as f64);
        Pass {
            cache: self,
            complete: false,
        }
    }

    pub(super) fn resolve(
        &self,
        row: &RawDocument,
        validate: impl FnOnce() -> Result<AgentConnectionRecoveryCandidate, SandboxStoreError>,
    ) -> Result<AgentConnectionRecoveryCandidate, SandboxStoreError> {
        let started = std::time::Instant::now();
        let now = Instant::now();
        let id = row.get_str("_id").ok().filter(|id| id.len() <= 512);
        let retained = row.as_bytes().len() <= MAX_ROW_BYTES;
        if let Some(id) = id {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = state.entries.get(id) {
                if now < entry.expires && entry.row.as_ref() == row.as_bytes() {
                    #[cfg(test)]
                    self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    metrics::counter!("cma_agent_discovery_validation_total", "outcome" => "reused").increment(1);
                    metrics::histogram!("cma_agent_discovery_validation_seconds", "outcome" => "reused")
                        .record(started.elapsed().as_secs_f64());
                    return Ok(entry.candidate.clone());
                }
            }
            // A changed or expired row must not leave an older positive entry
            // available after a failed validation or an oversized replacement.
            state.remove(id);
        }
        #[cfg(test)]
        self.validations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // read_agent/hydrate/from_agent are pure functions of this entire row.
        // Neither Mongo IO nor decoding runs while holding the cache mutex.
        let result = validate();
        let outcome = if result.is_ok() { "validated" } else { "error" };
        metrics::counter!("cma_agent_discovery_validation_total", "outcome" => outcome)
            .increment(1);
        metrics::histogram!("cma_agent_discovery_validation_seconds", "outcome" => outcome)
            .record(started.elapsed().as_secs_f64());
        let candidate = result?;
        if let Some(id) = id.filter(|_| retained) {
            let row = row.as_bytes().to_vec().into_boxed_slice();
            let charge = row.len()
                + id.len()
                + candidate.agent_id.as_str().len()
                + candidate.sandbox_id.as_str().len()
                + candidate.conversation_id.len();
            let entry = Entry {
                row,
                candidate: candidate.clone(),
                expires: now + MAX_AGE,
                charge,
            };
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.expire(now);
            state.remove(id);
            // Saturation fails open; never evict a live entry to churn through
            // an over-capacity sweep. Entry count also bounds map overhead.
            if state.entries.len() < MAX_ENTRIES && state.bytes + charge <= MAX_BYTES {
                state.bytes += charge;
                state.entries.insert(id.to_owned(), entry);
            } else {
                metrics::counter!("cma_agent_discovery_validation_total", "outcome" => "saturated")
                    .increment(1);
            }
            metrics::gauge!("cma_agent_discovery_cache_bytes").set(state.bytes as f64);
            metrics::gauge!("cma_agent_discovery_cache_entries").set(state.entries.len() as f64);
        } else {
            metrics::counter!("cma_agent_discovery_validation_total", "outcome" => "unretained")
                .increment(1);
        }
        Ok(candidate)
    }

    #[cfg(test)]
    pub(super) fn retained(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.entries.len(), state.bytes)
    }

    fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.entries.clear();
        state.bytes = METADATA_BYTES;
        metrics::gauge!("cma_agent_discovery_cache_bytes").set(state.bytes as f64);
        metrics::gauge!("cma_agent_discovery_cache_entries").set(0);
    }
}

/// Query errors, malformed rows and cancellation all discard reusable state.
pub(super) struct Pass<'a> {
    cache: &'a Observations,
    pub(super) complete: bool,
}
impl Drop for Pass<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.cache.clear();
        }
    }
}

impl super::MongoSandboxStore {
    pub(crate) fn with_agent_connection_observations(mut self, enabled: bool) -> Self {
        self.agent_connection_observations = enabled.then(Default::default);
        self
    }
}

#[cfg(test)]
mod tests;
