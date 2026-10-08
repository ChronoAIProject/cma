//! One event of a channel as the console keeps it, and which entries an
//! activity read returns.
use cmaeg_core::ids::ConsumerId;
use serde::Serialize;
use time::OffsetDateTime;

use super::rfc3339;
use crate::{
    console::observer::{CausationSummary, DeliveryClass},
    turns::observer::{DeliveryStep, Destination, LaunchClass},
};

/// What an entry is counted as, beyond its strings and consumers.
const ENTRY_OVERHEAD: usize = 512;
/// What each consumer's progress is counted as.
const CONSUMER_BYTES: usize = 96;

/// One event of a channel, as the console shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityEntry {
    pub seq: u64,
    #[serde(serialize_with = "rfc3339")]
    pub at: OffsetDateTime,
    /// 16 hex of SHA-256(event id); absent for a refusal that came before
    /// the event was identified.
    pub event_key: Option<String>,
    /// The source slot an admitted event arrived on (`telegram`,
    /// `relay_<platform>` or `custom`). A refusal may come before its slot
    /// is known, so it names the transport instead: `telegram`,
    /// `nyxid_relay` or `custom`.
    pub source: String,
    pub kind: &'static str,
    pub actor_masked: String,
    pub chat_masked: String,
    /// Attachment counts: image, audio, video, and file or other.
    pub attachments: [u8; 4],
    pub consumers: Vec<ConsumerProgress>,
    pub refusal: Option<String>,
    pub causation: Option<CausationSummary>,
}

/// One consumer's progress with an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumerProgress {
    pub consumer_id: ConsumerId,
    pub launch: Option<LaunchClass>,
    pub accepted_ms: Option<u32>,
    /// The latest delivery step and its outcome.
    pub delivery: Option<(DeliveryStep, DeliveryClass)>,
    pub tool_replied: bool,
    pub destination: Option<Destination>,
}

impl ConsumerProgress {
    pub(super) fn new(consumer_id: ConsumerId) -> Self {
        Self {
            consumer_id,
            launch: None,
            accepted_ms: None,
            delivery: None,
            tool_replied: false,
            destination: None,
        }
    }
}

impl ActivityEntry {
    /// The bytes this entry counts against the activity budget.
    pub fn weight(&self) -> usize {
        ENTRY_OVERHEAD
            + self.event_key.as_ref().map_or(0, String::len)
            + self.source.len()
            + self.actor_masked.len()
            + self.chat_masked.len()
            + self.refusal.as_ref().map_or(0, String::len)
            + self.consumers.len() * CONSUMER_BYTES
    }

    /// True for a refusal, a refused launch, a failure notice or a rejected
    /// delivery.
    pub fn failed(&self) -> bool {
        self.refusal.is_some()
            || self.consumers.iter().any(|progress| {
                progress.launch == Some(LaunchClass::Refused)
                    || matches!(
                        progress.delivery,
                        Some((DeliveryStep::Failure, _))
                            | Some((_, DeliveryClass::Rejected { .. }))
                    )
            })
    }

    pub(super) fn consumer(&mut self, id: &ConsumerId) -> &mut ConsumerProgress {
        let position = match self
            .consumers
            .iter()
            .position(|item| item.consumer_id == *id)
        {
            Some(position) => position,
            None => {
                self.consumers.push(ConsumerProgress::new(id.clone()));
                self.consumers.len() - 1
            }
        };
        &mut self.consumers[position]
    }
}

/// Which entries an activity read returns.
#[derive(Debug, Clone, Default)]
pub struct ActivityFilter {
    pub consumer: Option<ConsumerId>,
    /// A consumer's launch class; `Refused` also matches ingress refusals.
    pub outcome: Option<LaunchClass>,
    pub source: Option<String>,
}

impl ActivityFilter {
    pub(super) fn matches(&self, entry: &ActivityEntry) -> bool {
        let consumer = self.consumer.as_ref().is_none_or(|id| {
            entry
                .consumers
                .iter()
                .any(|progress| progress.consumer_id == *id)
        });
        let outcome = self.outcome.is_none_or(|class| {
            (class == LaunchClass::Refused && entry.refusal.is_some())
                || entry
                    .consumers
                    .iter()
                    .any(|progress| progress.launch == Some(class))
        });
        let source = self
            .source
            .as_ref()
            .is_none_or(|source| entry.source == *source);
        consumer && outcome && source
    }
}
