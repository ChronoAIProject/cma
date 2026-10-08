//! Per-channel activity rings and producer registries, under one global
//! byte budget.
//!
//! Each channel keeps at most `CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL` entries,
//! and every entry and producer counts its bytes against
//! `CMAEG_CONSOLE_ACTIVITY_MAX_BYTES`; beyond it the oldest entries of any
//! channel go first, so one busy channel cannot hold more than the budget.
//! Entries come only from authenticated events and never hold message text,
//! CloudEvent data, attachments, references or tokens.
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Mutex,
};

use cmaeg_core::ids::{ConsumerId, RecordId};
use serde::Serializer;
use time::OffsetDateTime;

use super::{
    observer::{ActivitySeed, CausationSummary, DeliveryClass, RefusalSeed},
    producers::{ProducerView, Producers, Seen},
};
use crate::{
    telemetry::names,
    turns::observer::{DeliveryStep, Destination, LaunchClass},
};

mod entry;

pub use entry::{ActivityEntry, ActivityFilter, ConsumerProgress};

/// A channel's recent traffic, as the channel list shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecentActivity {
    /// When the newest entry the ring holds was recorded.
    pub last_event: Option<OffsetDateTime>,
    /// Entries recorded since the cut-off.
    pub events: usize,
    /// Of those, the ones that failed.
    pub failures: usize,
}

/// How a known event changed.
pub enum Update {
    Launch {
        consumer: ConsumerId,
        outcome: LaunchClass,
        accepted_ms: Option<u32>,
    },
    Delivery {
        consumer: ConsumerId,
        step: DeliveryStep,
        outcome: DeliveryClass,
        destination: Destination,
    },
    Published(CausationSummary),
}

#[derive(Default)]
struct Channel {
    entries: VecDeque<(ActivityEntry, usize)>,
    producers: Producers,
}

#[derive(Default)]
struct State {
    channels: HashMap<RecordId, Channel>,
    /// Every entry's sequence number with its record, oldest first.
    order: BTreeMap<u64, RecordId>,
    bytes: usize,
    next_seq: u64,
}

/// The rings and registries of every channel.
pub struct ActivityRings {
    state: Mutex<State>,
    per_channel: usize,
    max_bytes: usize,
    producers_per_channel: usize,
}

/// The rings' totals, for gauges and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingTotals {
    pub entries: usize,
    pub bytes: usize,
    pub channels: usize,
}

impl ActivityRings {
    pub fn new(per_channel: usize, max_bytes: usize, producers_per_channel: usize) -> Self {
        Self {
            state: Mutex::default(),
            per_channel,
            max_bytes,
            producers_per_channel,
        }
    }

    /// Add an admitted event's entry.
    pub fn admitted(&self, record: RecordId, seed: ActivitySeed) {
        self.push(record, |seq| ActivityEntry {
            seq,
            at: seed.at,
            event_key: Some(seed.event_key),
            source: seed.source,
            kind: seed.kind,
            actor_masked: seed.actor_masked,
            chat_masked: seed.chat_masked,
            attachments: seed.attachments,
            consumers: Vec::new(),
            refusal: None,
            causation: None,
        });
    }

    /// Add an authenticated refusal's entry.
    pub fn refused(&self, record: RecordId, seed: RefusalSeed) {
        self.push(record, |seq| ActivityEntry {
            seq,
            at: seed.at,
            event_key: seed.event_key,
            source: seed.source.to_owned(),
            kind: "refusal",
            actor_masked: String::new(),
            chat_masked: String::new(),
            attachments: [0; 4],
            consumers: Vec::new(),
            refusal: Some(seed.reason),
            causation: None,
        });
    }

    /// Apply `update` to the newest entry of `record` with `event_key`; an
    /// event whose entry was already evicted is ignored.
    pub fn update(&self, record: &RecordId, event_key: &str, update: Update) {
        let mut state = self.lock();
        let Some(channel) = state.channels.get_mut(record) else {
            return;
        };
        let Some((entry, weight)) = channel
            .entries
            .iter_mut()
            .rev()
            .find(|(entry, _)| entry.event_key.as_deref() == Some(event_key))
        else {
            return;
        };
        match update {
            Update::Launch {
                consumer,
                outcome,
                accepted_ms,
            } => {
                let progress = entry.consumer(&consumer);
                progress.launch = Some(outcome);
                progress.accepted_ms = accepted_ms;
            }
            Update::Delivery {
                consumer,
                step,
                outcome,
                destination,
            } => {
                let progress = entry.consumer(&consumer);
                progress.tool_replied |=
                    step == DeliveryStep::ToolReply && outcome == DeliveryClass::Confirmed;
                progress.delivery = Some((step, outcome));
                progress.destination = Some(destination);
            }
            Update::Published(causation) => entry.causation = Some(causation),
        }
        let grown = entry.weight();
        let previous = std::mem::replace(weight, grown);
        state.bytes = state.bytes + grown - previous;
        self.enforce(&mut state);
    }

    /// Note a producer of `record`'s custom events.
    pub fn produced(&self, record: RecordId, seen: Seen) {
        let mut state = self.lock();
        let delta = state
            .channels
            .entry(record)
            .or_default()
            .producers
            .observe(seen, self.producers_per_channel);
        state.bytes = state.bytes.saturating_add_signed(delta);
        self.enforce(&mut state);
    }

    /// The newest entries of `record` matching `filter`, newest first.
    pub fn entries(
        &self,
        record: &RecordId,
        filter: &ActivityFilter,
        limit: usize,
    ) -> Vec<ActivityEntry> {
        let state = self.lock();
        state
            .channels
            .get(record)
            .map(|channel| {
                channel
                    .entries
                    .iter()
                    .rev()
                    .filter(|(entry, _)| filter.matches(entry))
                    .take(limit)
                    .map(|(entry, _)| entry.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The newest entry of `record` with `event_key`.
    pub fn event(&self, record: &RecordId, event_key: &str) -> Option<ActivityEntry> {
        let state = self.lock();
        state.channels.get(record).and_then(|channel| {
            channel
                .entries
                .iter()
                .rev()
                .find(|(entry, _)| entry.event_key.as_deref() == Some(event_key))
                .map(|(entry, _)| entry.clone())
        })
    }

    /// `record`'s newest entry time, and its entries and failures since
    /// `since`.
    pub fn recent(&self, record: &RecordId, since: OffsetDateTime) -> RecentActivity {
        let state = self.lock();
        let Some(channel) = state.channels.get(record) else {
            return RecentActivity::default();
        };
        let mut recent = RecentActivity {
            last_event: channel.entries.back().map(|(entry, _)| entry.at),
            ..RecentActivity::default()
        };
        for (entry, _) in channel.entries.iter().rev() {
            if entry.at < since {
                break;
            }
            recent.events += 1;
            recent.failures += usize::from(entry.failed());
        }
        recent
    }

    /// `record`'s producers, most recently seen first.
    pub fn producers(&self, record: &RecordId) -> Vec<ProducerView> {
        let state = self.lock();
        state
            .channels
            .get(record)
            .map(|channel| channel.producers.views())
            .unwrap_or_default()
    }

    pub fn totals(&self) -> RingTotals {
        let state = self.lock();
        RingTotals {
            entries: state.order.len(),
            bytes: state.bytes,
            channels: state.channels.len(),
        }
    }

    fn push(&self, record: RecordId, entry: impl FnOnce(u64) -> ActivityEntry) {
        let mut state = self.lock();
        state.next_seq += 1;
        let seq = state.next_seq;
        let entry = entry(seq);
        let weight = entry.weight();
        state.order.insert(seq, record.clone());
        state.bytes += weight;
        let channel = state.channels.entry(record).or_default();
        channel.entries.push_back((entry, weight));
        let mut freed = Vec::new();
        while channel.entries.len() > self.per_channel {
            if let Some((oldest, weight)) = channel.entries.pop_front() {
                freed.push((oldest.seq, weight));
            }
        }
        for (seq, weight) in freed {
            state.order.remove(&seq);
            state.bytes -= weight;
        }
        self.enforce(&mut state);
    }

    /// Evict the oldest entries of any channel, then the least recently
    /// seen producers, until the budget holds.
    fn enforce(&self, state: &mut State) {
        while state.bytes > self.max_bytes {
            if let Some((seq, record)) = state.order.pop_first() {
                // A channel's ring is in sequence order, so the globally
                // oldest entry is the front of its channel's ring.
                let freed = match state.channels.get_mut(&record) {
                    Some(channel)
                        if channel
                            .entries
                            .front()
                            .is_some_and(|(entry, _)| entry.seq == seq) =>
                    {
                        channel.entries.pop_front().map_or(0, |(_, weight)| weight)
                    }
                    _ => {
                        tracing::warn!("console activity order out of step with its rings");
                        0
                    }
                };
                state.bytes -= freed;
                drop_if_empty(state, &record);
                continue;
            }
            let oldest = state
                .channels
                .iter()
                .filter_map(|(record, channel)| Some((channel.producers.oldest()?, record)))
                .min_by_key(|(seen, _)| *seen)
                .map(|(_, record)| record.clone());
            let Some(record) = oldest else {
                break;
            };
            let freed = state
                .channels
                .get_mut(&record)
                .map_or(0, |channel| channel.producers.evict_oldest());
            state.bytes -= freed;
            drop_if_empty(state, &record);
        }
        metrics::gauge!(names::CONSOLE_ACTIVITY_BYTES).set(state.bytes as f64);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // Every mutation keeps the maps and the byte count consistent before
        // it can panic, so a poisoned lock still holds usable state.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn drop_if_empty(state: &mut State, record: &RecordId) {
    if state
        .channels
        .get(record)
        .is_some_and(|channel| channel.entries.is_empty() && channel.producers.is_empty())
    {
        state.channels.remove(record);
    }
}

pub(super) fn rfc3339<S: Serializer>(
    at: &OffsetDateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let text = at
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&text)
}
