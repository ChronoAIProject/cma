//! Nonblocking observation of bounded, already-masked routing facts.
//! Implementations never receive activities, provider bodies or problems.
mod summary;
pub use summary::{ActivitySummary, CausationSummary, Kind, RoutingSummary};

use crate::ingress::launch::LaunchOutcome;
use cmaeg_core::{
    channel::SourceId,
    ids::{ConsumerId, RecordId},
    routing::Trace,
};
use std::time::Duration;

pub trait ActivityObserver: Send + Sync {
    fn routed(&self, _record: &RecordId, _summary: &ActivitySummary, _trace: &Trace) {}
    fn outcome(&self, _record: &RecordId, _summary: &OutcomeSummary) {}
    fn refused(&self, _record: &RecordId, _summary: &RefusalSummary) {}
}

pub struct NoopObserver;
impl ActivityObserver for NoopObserver {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchClass {
    Accepted,
    Deferred,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeSummary {
    pub event_key: String,
    pub consumer: ConsumerId,
    pub class: LaunchClass,
    pub code: Option<&'static str>,
    pub provider_code: Option<String>,
    pub accepted_ms: Option<u32>,
}
impl OutcomeSummary {
    pub fn of(
        event: &str,
        consumer: ConsumerId,
        outcome: &LaunchOutcome,
        elapsed: Duration,
    ) -> Self {
        let (class, code, provider_code) = match outcome {
            LaunchOutcome::Accepted { .. } => (LaunchClass::Accepted, None, None),
            LaunchOutcome::Deferred => (LaunchClass::Deferred, None, None),
            LaunchOutcome::Refused(refusal) => (
                LaunchClass::Refused,
                Some(refusal.code),
                crate::delivery::provider_code(refusal.provider_code.clone()),
            ),
        };
        Self {
            event_key: crate::telemetry::channel_hash(event),
            consumer,
            class,
            code,
            provider_code,
            accepted_ms: (class == LaunchClass::Accepted)
                .then(|| u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusalSummary {
    pub at: time::OffsetDateTime,
    pub event_key: Option<String>,
    pub source: SourceId,
    pub code: &'static str,
}

#[cfg(test)]
#[path = "observer_tests.rs"]
mod tests;
