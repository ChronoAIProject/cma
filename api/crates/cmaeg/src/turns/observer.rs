//! What happens to a channel's events, reported to an observer.
//!
//! Ingress, launch and delivery call an [`ActivityObserver`] at each step an
//! owner may want to inspect. Every call is synchronous and must return at
//! once: the console's observer only queues with `try_send`, and the
//! [`NoopObserver`] composed while the console is off does nothing at all.
//! The arguments are the raw facts the caller already holds; an observer that
//! keeps anything masks it before it leaves the call, so no observer can make
//! a hot path wait or retain message content.
use std::{sync::Arc, time::Instant};

use cmaeg_core::{
    activity::Activity,
    ids::{ConsumerId, EventId, RecordId},
    routing::Trace,
};
use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::sync::OwnedSemaphorePermit;

use crate::{
    delivery::intents::DeliveryOutcome,
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
};

/// How a consumer's launch ended at ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchClass {
    Accepted,
    Deferred,
    Refused,
}

impl LaunchClass {
    pub fn of(outcome: &LaunchOutcome) -> Self {
        match outcome {
            LaunchOutcome::Accepted { .. } | LaunchOutcome::Admitted { .. } => Self::Accepted,
            LaunchOutcome::Deferred => Self::Deferred,
            LaunchOutcome::Refused(_) => Self::Refused,
        }
    }
}

/// Which delivery step sent a message for an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStep {
    /// The progress acknowledgement of a long turn.
    Ack,
    /// A "still working" message after the acknowledgement.
    Heartbeat,
    /// The turn's own answer, sent automatically.
    Answer,
    /// A reply the agent sent through its reply tool.
    ToolReply,
    /// The failure notice of a failed or incomplete turn.
    Failure,
    /// A notice sent to the channel's default reply destination.
    DefaultReply,
}

/// Where a delivery went: the event's origin, or the channel's default reply
/// destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Origin,
    Default,
}

/// The facts of an event an agent published into a channel.
#[derive(Debug, Clone, Copy)]
pub struct Causation<'a> {
    /// The event the publishing turn was serving.
    pub parent: &'a EventId,
    /// How many publications deep the new event is.
    pub hop: u8,
    /// The consumer whose agent published it.
    pub publisher: &'a ConsumerId,
}

/// The facts of one authenticated custom event's producer.
#[derive(Debug, Clone, Copy)]
pub struct ProducerFacts<'a> {
    /// The event's raw `ce.source`; an observer strips it before keeping it.
    pub source: &'a str,
    /// The credential that authenticated it, as
    /// [`crate::ingress::custom::ProducerHint`] displays it.
    pub hint: &'a str,
    /// The event's `ce.type`.
    pub ce_type: &'a str,
    pub outcome: LaunchClass,
}

/// Receives the steps of a channel's events. Every method defaults to doing
/// nothing, so an implementation observes only what it needs.
pub trait ActivityObserver: Send + Sync {
    /// An authenticated event is about to launch.
    fn admitted(&self, _record: &RecordId, _activity: &Activity) {}

    /// Routing decided an authenticated event's consumers, and it is about
    /// to launch: to `activity.routing`'s targets, or, when that is absent,
    /// on the legacy path the kill switch's shadow decision allowed.
    /// `trace` is how the decision was reached; it is never sealed or sent
    /// to a provider. The ingress that routed the event reports it once,
    /// before launching; a launcher that also reports the launch through
    /// [`ActivityObserver::admitted`] describes the same admission.
    fn routed(&self, _record: &RecordId, _activity: &Activity, _trace: &Trace) {}

    /// Routing refused an authenticated event. `reason` is the stable label
    /// of the routing error and `trace` what `routing::explain` recorded for
    /// it, including the kill switch's shadow decision.
    fn route_refused(
        &self,
        _record: &RecordId,
        _activity: &Activity,
        _reason: &str,
        _trace: &Trace,
    ) {
    }

    /// An authenticated event was refused before launching. `event_id` is
    /// absent when the refusal came before the event was identified.
    fn refused(
        &self,
        _record: &RecordId,
        _source: &'static str,
        _event_id: Option<&EventId>,
        _reason: &str,
    ) {
    }

    /// A consumer's launch ended; `accepted` is the time from receipt to
    /// that end.
    fn launched(
        &self,
        _record: &RecordId,
        _event_id: &EventId,
        _consumer: &ConsumerId,
        _outcome: LaunchClass,
        _accepted: std::time::Duration,
    ) {
    }

    /// A delivery step for an event finished with `outcome`.
    fn delivered(
        &self,
        _record: &RecordId,
        _event_id: &EventId,
        _consumer: &ConsumerId,
        _step: DeliveryStep,
        _outcome: &DeliveryOutcome,
        _destination: Destination,
    ) {
    }

    /// An agent published `event_id` into the channel.
    fn published(&self, _record: &RecordId, _event_id: &EventId, _causation: Causation<'_>) {}

    /// An authenticated custom event named its producer.
    fn produced(&self, _record: &RecordId, _producer: ProducerFacts<'_>) {}

    /// A request failed ingress authentication. Only a bounded counter may
    /// record it: anyone who knows a channel URL can send these.
    fn unauthenticated(&self, _source: &'static str) {}
}

/// The `default` consumer id, built once: legacy launches and deliveries
/// serve only it, and reporting them must not allocate.
pub fn default_consumer() -> &'static ConsumerId {
    static DEFAULT: std::sync::OnceLock<ConsumerId> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(ConsumerId::default_consumer)
}

/// Observes nothing: the observer composed while the console is off.
pub struct NoopObserver;

impl ActivityObserver for NoopObserver {}

/// A launcher that reports each launch it passes to `inner`: the event as
/// admitted before it launches, then the default consumer's outcome. It is
/// composed only while an observer is configured, so a gateway without one
/// launches exactly as before.
pub struct ObservedLauncher {
    inner: Arc<dyn TurnLauncher>,
    observer: Arc<dyn ActivityObserver>,
}

impl ObservedLauncher {
    pub fn new(inner: Arc<dyn TurnLauncher>, observer: Arc<dyn ActivityObserver>) -> Self {
        Self { inner, observer }
    }

    /// Report the admission and keep what the outcome report needs, since
    /// the request itself moves into the launch.
    fn admit(&self, request: &LaunchRequest) -> Launched {
        let record = request.channel.definition.record_id.clone();
        self.observer.admitted(&record, &request.activity);
        Launched {
            record,
            event_id: request.activity.event_id.clone(),
            consumer: request
                .consumer
                .clone()
                .unwrap_or_else(|| default_consumer().clone()),
            received_at: request.received_at,
        }
    }

    fn report(&self, launched: &Launched, outcome: &LaunchOutcome) {
        self.observer.launched(
            &launched.record,
            &launched.event_id,
            &launched.consumer,
            LaunchClass::of(outcome),
            launched.received_at.elapsed(),
        );
    }
}

struct Launched {
    record: RecordId,
    event_id: EventId,
    consumer: ConsumerId,
    received_at: Instant,
}

impl TurnLauncher for ObservedLauncher {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        let launched = self.admit(&request);
        Box::pin(async move {
            let outcome = self.inner.launch(request).await;
            self.report(&launched, &outcome);
            outcome
        })
    }

    fn launch_holding(
        &self,
        request: LaunchRequest,
        hold: OwnedSemaphorePermit,
    ) -> BoxFuture<'_, LaunchOutcome> {
        let launched = self.admit(&request);
        Box::pin(async move {
            let outcome = self.inner.launch_holding(request, hold).await;
            self.report(&launched, &outcome);
            outcome
        })
    }
}
