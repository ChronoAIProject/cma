//! The console's observer: every hot-path report becomes a masked
//! [`Observation`] queued with `try_send`, and one console task drains the
//! queue into the console's memory.
//!
//! A full queue drops the observation and counts it in
//! `cmaeg_console_observer_dropped_total`; no caller ever waits. Masking
//! happens here, while the observation is built, so raw actor and chat ids,
//! event ids, `ce.source` userinfo and query strings never enter the queue.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use cmaeg_core::{
    activity::{Activity, ActivityKind, AttachmentKind, SourceKind},
    ids::{ChannelId, ConsumerId, EventId, RecordId},
};
use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::mpsc::{self, error::TrySendError};

use super::{activity::rfc3339, mask};
use crate::{
    channels::observer::ChannelIndexObserver,
    delivery::intents::DeliveryOutcome,
    ingress::launch::TurnLauncher,
    telemetry::names,
    turns::observer::{
        ActivityObserver, Causation, DeliveryStep, Destination, LaunchClass, NoopObserver,
        ObservedLauncher, ProducerFacts,
    },
};

/// Observations the queue holds before it drops new ones.
pub const QUEUE_CAPACITY: usize = 4_096;
/// Bytes a producer's `ce.type` keeps.
const CE_TYPE_BYTES: usize = 128;

/// One masked report from a hot path.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum Observation {
    /// An authenticated event was admitted and is launching.
    Routed {
        record: RecordId,
        entry_seed: ActivitySeed,
    },
    /// An authenticated event was refused before launching.
    Refused {
        record: RecordId,
        seed: RefusalSeed,
    },
    Launch {
        record: RecordId,
        event_key: String,
        consumer: ConsumerId,
        outcome: LaunchClass,
        accepted_ms: Option<u32>,
    },
    Delivery {
        record: RecordId,
        event_key: String,
        consumer: ConsumerId,
        step: DeliveryStep,
        outcome: DeliveryClass,
        destination: Destination,
    },
    Published {
        record: RecordId,
        event_key: String,
        causation: CausationSummary,
    },
    Producer {
        record: RecordId,
        source_uri: String,
        hint: String,
        ce_type: String,
        outcome: LaunchClass,
    },
    ChannelId {
        record: RecordId,
        id: ChannelId,
    },
    IndexChanged {
        record: RecordId,
        owner: String,
        version: u64,
    },
    IndexRemoved {
        record: RecordId,
    },
}

/// What an admitted event's activity entry starts from: identifiers masked,
/// and no message text, CloudEvent data or attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivitySeed {
    #[serde(serialize_with = "rfc3339")]
    pub at: OffsetDateTime,
    pub event_key: String,
    /// The source kind, with the relayed platform: `telegram`,
    /// `nyxid_relay:lark` or `custom`.
    pub source: String,
    pub kind: &'static str,
    pub actor_masked: String,
    pub chat_masked: String,
    /// Attachment counts by kind: image, audio, video, and file or other.
    pub attachments: [u8; 4],
}

impl ActivitySeed {
    pub fn of(activity: &Activity) -> Self {
        let (kind, attachments) = match &activity.kind {
            ActivityKind::Message { attachments, .. } => ("message", counts(attachments)),
            ActivityKind::Interaction(_) => ("interaction", [0; 4]),
            ActivityKind::Command { .. } => ("command", [0; 4]),
            ActivityKind::Event { attachments, .. } => ("event", counts(attachments)),
        };
        Self {
            at: OffsetDateTime::now_utc(),
            event_key: mask::event_key(activity.event_id.as_str()),
            source: source_label(&activity.source),
            kind,
            actor_masked: mask::actor(&activity.actor.id),
            chat_masked: mask::chat(&activity.conversation.id),
            attachments,
        }
    }
}

/// An authenticated refusal's entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusalSeed {
    #[serde(serialize_with = "rfc3339")]
    pub at: OffsetDateTime,
    /// Absent when the refusal came before the event was identified.
    pub event_key: Option<String>,
    pub source: &'static str,
    pub reason: String,
}

/// A delivery's outcome, without any platform text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeliveryClass {
    Confirmed,
    Unknown,
    Rejected { reason: String },
    RateLimited,
    Unsupported,
}

impl DeliveryClass {
    pub fn of(outcome: &DeliveryOutcome) -> Self {
        match outcome {
            DeliveryOutcome::Confirmed { .. } => Self::Confirmed,
            DeliveryOutcome::Unknown { .. } => Self::Unknown,
            DeliveryOutcome::Rejected { reason } => Self::Rejected {
                reason: mask::label(reason),
            },
            DeliveryOutcome::RateLimited { .. } => Self::RateLimited,
            DeliveryOutcome::Unsupported => Self::Unsupported,
        }
    }
}

/// Where a published event came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CausationSummary {
    pub parent_key: String,
    pub hop: u8,
    pub published_by: ConsumerId,
}

/// Queues masked observations for the console. Cloning shares the queue.
#[derive(Clone)]
pub struct ConsoleObserver {
    queue: mpsc::Sender<Observation>,
    dropped: Arc<AtomicU64>,
}

/// The receiving end of the console's observation queue.
pub struct ObservationQueue(mpsc::Receiver<Observation>);

impl ConsoleObserver {
    /// An observer and its queue of [`QUEUE_CAPACITY`] observations.
    pub fn new() -> (Self, ObservationQueue) {
        let (queue, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let observer = Self {
            queue,
            dropped: Arc::default(),
        };
        (observer, ObservationQueue(receiver))
    }

    /// Observations this observer dropped because the queue was full or
    /// closed.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn send(&self, observation: Observation) {
        if let Err(error) = self.queue.try_send(observation) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            metrics::counter!(names::CONSOLE_OBSERVER_DROPPED).increment(1);
            let reason = match error {
                TrySendError::Full(_) => "full",
                TrySendError::Closed(_) => "closed",
            };
            tracing::debug!(reason, "console observation dropped");
        }
    }
}

impl ObservationQueue {
    /// Apply every observation in arrival order until each observer is gone.
    pub async fn drain(mut self, mut apply: impl FnMut(Observation)) {
        while let Some(observation) = self.0.recv().await {
            apply(observation);
        }
    }

    /// The next observation if one is queued, for tests that hold the
    /// drainer back.
    #[cfg(test)]
    pub(crate) fn try_next(&mut self) -> Option<Observation> {
        self.0.try_recv().ok()
    }
}

impl ActivityObserver for ConsoleObserver {
    fn admitted(&self, record: &RecordId, activity: &Activity) {
        self.send(Observation::Routed {
            record: record.clone(),
            entry_seed: ActivitySeed::of(activity),
        });
    }

    fn refused(
        &self,
        record: &RecordId,
        source: &'static str,
        event_id: Option<&EventId>,
        reason: &str,
    ) {
        self.send(Observation::Refused {
            record: record.clone(),
            seed: RefusalSeed {
                at: OffsetDateTime::now_utc(),
                event_key: event_id.map(|id| mask::event_key(id.as_str())),
                source,
                reason: mask::label(reason),
            },
        });
    }

    fn launched(
        &self,
        record: &RecordId,
        event_id: &EventId,
        consumer: &ConsumerId,
        outcome: LaunchClass,
        accepted: std::time::Duration,
    ) {
        let accepted_ms = (outcome == LaunchClass::Accepted)
            .then(|| u32::try_from(accepted.as_millis()).unwrap_or(u32::MAX));
        self.send(Observation::Launch {
            record: record.clone(),
            event_key: mask::event_key(event_id.as_str()),
            consumer: consumer.clone(),
            outcome,
            accepted_ms,
        });
    }

    fn delivered(
        &self,
        record: &RecordId,
        event_id: &EventId,
        consumer: &ConsumerId,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        destination: Destination,
    ) {
        self.send(Observation::Delivery {
            record: record.clone(),
            event_key: mask::event_key(event_id.as_str()),
            consumer: consumer.clone(),
            step,
            outcome: DeliveryClass::of(outcome),
            destination,
        });
    }

    fn published(&self, record: &RecordId, event_id: &EventId, causation: Causation<'_>) {
        self.send(Observation::Published {
            record: record.clone(),
            event_key: mask::event_key(event_id.as_str()),
            causation: CausationSummary {
                parent_key: mask::event_key(causation.parent.as_str()),
                hop: causation.hop,
                published_by: causation.publisher.clone(),
            },
        });
    }

    fn produced(&self, record: &RecordId, producer: ProducerFacts<'_>) {
        self.send(Observation::Producer {
            record: record.clone(),
            source_uri: mask::source_uri(producer.source),
            hint: mask::truncate(producer.hint, CE_TYPE_BYTES),
            ce_type: mask::truncate(producer.ce_type, CE_TYPE_BYTES),
            outcome: producer.outcome,
        });
    }

    fn unauthenticated(&self, source: &'static str) {
        metrics::counter!(names::CONSOLE_UNAUTHENTICATED, names::SOURCE => source).increment(1);
    }
}

impl ChannelIndexObserver for ConsoleObserver {
    fn changed(&self, record: &RecordId, holder: &str, version: u64) {
        self.send(Observation::IndexChanged {
            record: record.clone(),
            owner: holder.to_owned(),
            version,
        });
    }

    fn removed(&self, record: &RecordId) {
        self.send(Observation::IndexRemoved {
            record: record.clone(),
        });
    }

    fn observed(&self, record: &RecordId, id: &ChannelId) {
        self.send(Observation::ChannelId {
            record: record.clone(),
            id: id.clone(),
        });
    }
}

/// The observers composed into the gateway's hot paths: the console's while
/// `CMAEG_CONSOLE_ENABLED` is on, otherwise [`NoopObserver`]s with no queue,
/// so a gateway without the console allocates nothing for it.
pub struct Observers {
    pub activity: Arc<dyn ActivityObserver>,
    pub index: Arc<dyn ChannelIndexObserver>,
    /// The console's queue, for its drainer to take; `None` while the
    /// console is off.
    pub queue: Option<ObservationQueue>,
    /// The console's observer itself, for its drop count; `None` while the
    /// console is off.
    pub console: Option<ConsoleObserver>,
}

impl Observers {
    pub fn new(console_enabled: bool) -> Self {
        if !console_enabled {
            return Self {
                activity: Arc::new(NoopObserver),
                index: Arc::new(NoopObserver),
                queue: None,
                console: None,
            };
        }
        let (observer, queue) = ConsoleObserver::new();
        Self {
            activity: Arc::new(observer.clone()),
            index: Arc::new(observer.clone()),
            queue: Some(queue),
            console: Some(observer),
        }
    }

    /// `inner`, reporting each launch while the console is on; `inner`
    /// itself otherwise, so launches take exactly the path they took before.
    pub fn launcher(&self, inner: Arc<dyn TurnLauncher>) -> Arc<dyn TurnLauncher> {
        if self.console.is_none() {
            return inner;
        }
        Arc::new(ObservedLauncher::new(inner, self.activity.clone()))
    }
}

/// `telegram`, `nyxid_relay:<platform>` or `custom`.
fn source_label(source: &SourceKind) -> String {
    match source {
        SourceKind::Telegram { .. } => "telegram".to_owned(),
        SourceKind::NyxidRelay { platform, .. } => format!("nyxid_relay:{}", platform.as_str()),
        SourceKind::Custom => "custom".to_owned(),
    }
}

/// Image, audio, video, and everything else.
fn counts(attachments: &[cmaeg_core::activity::Attachment]) -> [u8; 4] {
    let mut counts = [0u8; 4];
    for attachment in attachments {
        let slot = match attachment.kind {
            AttachmentKind::Image => 0,
            AttachmentKind::Audio => 1,
            AttachmentKind::Video => 2,
            AttachmentKind::File | AttachmentKind::Other => 3,
        };
        counts[slot] = counts[slot].saturating_add(1);
    }
    counts
}

#[cfg(test)]
#[path = "observer_tests.rs"]
mod tests;
