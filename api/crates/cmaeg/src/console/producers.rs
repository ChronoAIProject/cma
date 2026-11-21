//! Which custom producers a channel has seen, by their stripped `ce.source`.
//!
//! Producer identity is advisory: it proves only that the sender held one of
//! the channel's ingress credentials, and in binary mode under HMAC the
//! `ce-source` header is not covered by the signature. The console says so
//! wherever it shows a producer.
use serde::Serialize;
use time::OffsetDateTime;

use crate::turns::observer::LaunchClass;

/// What one producer entry is counted as, beyond its strings.
const ENTRY_OVERHEAD: usize = 160;

/// One producer of a channel's custom events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProducerView {
    /// `ce.source` without userinfo, query or fragment, at most 256 bytes.
    pub source_uri: String,
    /// The credential that authenticated its latest event: `hmac:<key_id>`
    /// or `nyxid_key:#<index>`.
    pub hint: String,
    #[serde(serialize_with = "super::activity::rfc3339")]
    pub first_seen: OffsetDateTime,
    #[serde(serialize_with = "super::activity::rfc3339")]
    pub last_seen: OffsetDateTime,
    pub count: u64,
    pub last_type: String,
    pub last_outcome: LaunchClass,
}

impl ProducerView {
    /// The bytes this entry counts against the activity budget.
    pub fn weight(&self) -> usize {
        ENTRY_OVERHEAD + self.source_uri.len() + self.hint.len() + self.last_type.len()
    }
}

/// One channel's producers, least recently seen first out.
#[derive(Debug, Default)]
pub struct Producers {
    /// Ordered from least to most recently seen.
    entries: Vec<ProducerView>,
}

/// A producer's latest event.
pub struct Seen {
    pub source_uri: String,
    pub hint: String,
    pub ce_type: String,
    pub outcome: LaunchClass,
    pub at: OffsetDateTime,
}

impl Producers {
    /// Record `seen`, keeping at most `max` producers. Returns the change in
    /// counted bytes.
    pub fn observe(&mut self, seen: Seen, max: usize) -> isize {
        let mut delta = 0isize;
        let entry = match self
            .entries
            .iter()
            .position(|entry| entry.source_uri == seen.source_uri)
        {
            Some(position) => {
                let mut entry = self.entries.remove(position);
                delta -= entry.weight() as isize;
                entry.hint = seen.hint;
                entry.last_seen = seen.at;
                entry.count = entry.count.saturating_add(1);
                entry.last_type = seen.ce_type;
                entry.last_outcome = seen.outcome;
                entry
            }
            None => ProducerView {
                source_uri: seen.source_uri,
                hint: seen.hint,
                first_seen: seen.at,
                last_seen: seen.at,
                count: 1,
                last_type: seen.ce_type,
                last_outcome: seen.outcome,
            },
        };
        delta += entry.weight() as isize;
        self.entries.push(entry);
        while self.entries.len() > max {
            let evicted = self.entries.remove(0);
            delta -= evicted.weight() as isize;
        }
        delta
    }

    /// Drop the least recently seen producer; the bytes it freed.
    pub fn evict_oldest(&mut self) -> usize {
        if self.entries.is_empty() {
            return 0;
        }
        self.entries.remove(0).weight()
    }

    /// When the least recently seen producer was seen.
    pub fn oldest(&self) -> Option<OffsetDateTime> {
        self.entries.first().map(|entry| entry.last_seen)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Most recently seen first.
    pub fn views(&self) -> Vec<ProducerView> {
        self.entries.iter().rev().cloned().collect()
    }
}
