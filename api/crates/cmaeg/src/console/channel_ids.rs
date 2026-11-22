//! Channel ids the console can display, by record.
//!
//! A channel id is a pointer sealed with a random nonce, so it cannot be
//! recomputed from its record, and sealing a new one would show a different
//! id than the owner's integrations use. The console therefore only shows
//! ids it has seen: a recoverable creation's `creation.channel_id`, ids the
//! resolver restored in this process, and ids an owner pastes. The map is
//! display-only and least recently used first out; mutations address
//! records directly, and an evicted id is relearned from traffic.
use cmaeg_core::{
    ids::{ChannelId, RecordId},
    seal::MasterKeyRing,
};
use moka::sync::Cache;

use crate::records::RecordClient;

pub struct ChannelIds {
    cache: Cache<RecordId, ChannelId>,
}

/// Why a pasted channel id was not linked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// Not a channel id this gateway sealed.
    Malformed,
    /// It opens to another record, or to a record outside the configured
    /// CMA.
    Mismatch,
    /// The record is not the reader's.
    NotFound,
}

impl ChannelIds {
    pub fn new(max: u64) -> Self {
        Self {
            cache: Cache::new(max),
        }
    }

    pub fn get(&self, record: &RecordId) -> Option<ChannelId> {
        self.cache.get(record)
    }

    pub fn learn(&self, record: RecordId, id: ChannelId) {
        self.cache.insert(record, id);
    }
}

/// Check that `id` opens, with `keys`, to `record` stored in the CMA that
/// `records` reads; the caller has already checked that the record is the
/// reader's.
pub fn opens_to(
    id: &ChannelId,
    record: &RecordId,
    keys: &MasterKeyRing,
    records: &RecordClient,
) -> Result<(), LinkError> {
    let pointer = id.open(keys).map_err(|_| LinkError::Malformed)?.payload;
    if pointer.record_id != *record || !records.names_record(&pointer.record_url, record.as_str()) {
        return Err(LinkError::Mismatch);
    }
    Ok(())
}
