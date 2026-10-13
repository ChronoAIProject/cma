//! The owner index: which owner each channel record belongs to, at which
//! version, from CMA's own record heads.
//!
//! Ownership is immutable and the heads come from CMA's listing, so the index
//! is the console's authority for "may this owner see this record". It is
//! bounded by `CMAEG_CONSOLE_INDEX_MAX_RECORDS`; beyond the bound new records
//! are not indexed and the index is marked partial. Every owner-scoped read
//! takes an [`Authorized`], so no caller can list another owner's records.
use std::{
    collections::{BTreeMap, HashMap},
    sync::RwLock,
};

use cmaeg_core::ids::RecordId;

use super::authority::Authorized;
use crate::{records::RecordHead, telemetry::names};

/// A record's head: its owner and version, and when it was last written.
#[derive(Debug, Clone)]
struct Head {
    owner: String,
    version: u64,
    /// The index generation of the last write, so a listing that started
    /// earlier never removes a record written while it ran.
    stamp: u64,
}

#[derive(Default)]
struct State {
    by_owner: HashMap<String, BTreeMap<RecordId, u64>>,
    by_record: HashMap<RecordId, Head>,
    partial: bool,
    loading: Option<u64>,
    generation: u64,
}

pub struct OwnerIndex {
    state: RwLock<State>,
    max_records: usize,
}

/// The index's size and progress, for gauges and the console's banners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStatus {
    pub records: usize,
    pub partial: bool,
    pub loading: Option<u64>,
}

impl OwnerIndex {
    pub fn new(max_records: usize) -> Self {
        Self {
            state: RwLock::default(),
            max_records,
        }
    }

    /// Record `record`'s head: owned by `owner`, at `version` or a later
    /// version already seen. A record beyond the bound is not indexed, and
    /// the index becomes partial.
    pub fn upsert(&self, record: RecordId, owner: String, version: u64) {
        let mut state = self.write();
        state.generation += 1;
        let stamp = state.generation;
        let mut version = version;
        match state
            .by_record
            .get(&record)
            .map(|head| (head.owner.clone(), head.version))
        {
            Some((previous, _)) if previous != owner => {
                // Ownership never changes; a different owner means CMA's head
                // is authoritative over what this index last saw.
                tracing::warn!("console index record changed owner; following CMA");
                unlink(&mut state, &previous, &record);
            }
            // A page listed before a write may arrive after it.
            Some((_, known)) => version = version.max(known),
            None if state.by_record.len() >= self.max_records => {
                if !state.partial {
                    tracing::warn!(
                        max_records = self.max_records,
                        "console index is full; further channels are not indexed"
                    );
                }
                state.partial = true;
                publish(&state);
                return;
            }
            None => {}
        }
        state
            .by_owner
            .entry(owner.clone())
            .or_default()
            .insert(record.clone(), version);
        state.by_record.insert(
            record,
            Head {
                owner,
                version,
                stamp,
            },
        );
        publish(&state);
    }

    pub fn remove(&self, record: &RecordId) {
        let mut state = self.write();
        if let Some(head) = state.by_record.remove(record) {
            unlink(&mut state, &head.owner, record);
        }
        publish(&state);
    }

    /// `record`'s version when `auth` owns it; `None` for a record that is
    /// not indexed or belongs to anyone else.
    pub fn version_for(&self, auth: &Authorized, record: &RecordId) -> Option<u64> {
        self.read()
            .by_record
            .get(record)
            .filter(|head| head.owner == auth.owner())
            .map(|head| head.version)
    }

    /// Every record `auth` owns with its version, in record id order.
    pub fn records_for(&self, auth: &Authorized) -> Vec<(RecordId, u64)> {
        self.read()
            .by_owner
            .get(auth.owner())
            .map(|owned| {
                owned
                    .iter()
                    .map(|(record, version)| (record.clone(), *version))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The generation a listing starts at; see [`OwnerIndex::retire`].
    pub fn begin(&self, full: bool) -> u64 {
        let mut state = self.write();
        if full {
            state.loading = Some(0);
        }
        state.generation
    }

    /// Progress of a running full listing.
    pub fn progress(&self, seen: u64) {
        self.write().loading = Some(seen);
    }

    /// After a complete listing that began at `since`: drop the records it
    /// did not see and nothing wrote since, of every owner (`None`) or of
    /// the one `scope` lists.
    pub fn retire(&self, since: u64, scope: Option<&Authorized>) {
        let mut state = self.write();
        let stale: Vec<(RecordId, String)> = state
            .by_record
            .iter()
            .filter(|(_, head)| head.stamp <= since)
            .filter(|(_, head)| scope.is_none_or(|auth| head.owner == auth.owner()))
            .map(|(record, head)| (record.clone(), head.owner.clone()))
            .collect();
        for (record, owner) in stale {
            state.by_record.remove(&record);
            unlink(&mut state, &owner, &record);
        }
        if scope.is_none() {
            state.loading = None;
        }
        publish(&state);
    }

    /// A full listing failed: it is no longer loading, and nothing is
    /// removed on its partial evidence.
    pub fn abandon(&self) {
        self.write().loading = None;
    }

    pub fn status(&self) -> IndexStatus {
        let state = self.read();
        IndexStatus {
            records: state.by_record.len(),
            partial: state.partial,
            loading: state.loading,
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        // A panic while holding the lock cannot leave a half-written head:
        // every write completes its two maps before it can fail.
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A listed head as an index entry; `None` for an id CMA could not have
/// stored, which is skipped.
pub fn entry(head: &RecordHead) -> Option<(RecordId, String, u64)> {
    match RecordId::new(head.record_id.as_str()) {
        Ok(record) => Some((record, head.owner.clone(), head.version)),
        Err(_) => {
            tracing::warn!("console index skipped a record head with an invalid id");
            None
        }
    }
}

/// Drop `record` from the records `holder` owns.
fn unlink(state: &mut State, holder: &str, record: &RecordId) {
    if let Some(owned) = state.by_owner.get_mut(holder) {
        owned.remove(record);
        if owned.is_empty() {
            state.by_owner.remove(holder);
        }
    }
}

fn publish(state: &State) {
    metrics::gauge!(names::CONSOLE_INDEX_RECORDS).set(state.by_record.len() as f64);
    metrics::gauge!(names::CONSOLE_INDEX_PARTIAL).set(if state.partial { 1.0 } else { 0.0 });
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
