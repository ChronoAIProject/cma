//! Content-free, process-local public stream observations.
use futures_util::Stream;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock,
    },
    task::{Context, Poll as TaskPoll},
};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Agui,
    AgentEventsV2,
    Responses,
    ResponsesV2,
    LaunchEvents,
}
const LABELS: [&str; 5] = [
    "agui",
    "agent_events_v2",
    "responses",
    "responses_v2",
    "launch_events",
];
const OUTCOMES: [&str; 3] = ["quiet", "output", "failed"];

#[derive(Clone, Copy)]
pub(crate) enum AgentWake {
    Live,
    Head,
    Fallback,
    Authorization,
}

pub(crate) fn agent_wake(source: AgentWake) {
    let source = match source {
        AgentWake::Live => "live",
        AgentWake::Head => "head",
        AgentWake::Fallback => "fallback",
        AgentWake::Authorization => "authorization",
    };
    metrics::counter!("cma_agent_event_stream_wakes_total", "source" => source).increment(1);
}

pub(crate) fn agent_live_delivered(published: tokio::time::Instant) {
    metrics::histogram!("cma_agent_event_stream_delivery_lag_seconds")
        .record(published.elapsed().as_secs_f64());
}

#[derive(Default)]
struct Counters {
    active: [AtomicU64; LABELS.len()],
    polls: [[AtomicU64; OUTCOMES.len()]; LABELS.len()],
}
static COUNTERS: LazyLock<Arc<Counters>> = LazyLock::new(Arc::default);

fn counters() -> Arc<Counters> {
    #[cfg(test)]
    if let Ok(counters) = TEST_COUNTERS.try_with(Arc::clone) {
        return counters;
    }
    Arc::clone(&COUNTERS)
}

fn increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_add(1))
    });
}

/// Called by the serving process's bounded diagnostics timer, never per frame.
pub fn emit() {
    counters().emit();
}
impl Counters {
    fn emit(&self) {
        for (index, kind) in LABELS.iter().enumerate() {
            metrics::gauge!("cma_public_streams_active", "stream_kind" => *kind)
                .set(self.active[index].load(Ordering::Relaxed) as f64);
            tracing::info!(parent: None, metric = "cma_public_streams_active", stream_kind = kind,
                value = self.active[index].load(Ordering::Relaxed), "public stream gauge");
            for (outcome, label) in OUTCOMES.iter().enumerate() {
                metrics::counter!("cma_public_stream_polls_total", "stream_kind" => *kind, "outcome" => *label).absolute(self.polls[index][outcome].load(Ordering::Relaxed));
                tracing::info!(parent: None, metric = "cma_public_stream_polls_total", stream_kind = kind,
                    outcome = label, value = self.polls[index][outcome].load(Ordering::Relaxed),
                    "public stream poll counter");
            }
        }
    }
}

/// One refresh of an established stream, excluding initial replay and keepalive.
/// Dropping an unfinished observation counts failure, including cancellation.
pub(crate) struct Poll {
    counters: Arc<Counters>,
    kind: Kind,
    outcome: usize,
}
impl Poll {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            counters: counters(),
            kind,
            outcome: 2,
        }
    }
    pub(crate) fn complete(mut self, output: bool) {
        self.outcome = usize::from(output);
    }
}
impl Drop for Poll {
    fn drop(&mut self) {
        increment(&self.counters.polls[self.kind as usize][self.outcome]);
    }
}

struct Active {
    counters: Arc<Counters>,
    kind: Kind,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.counters.active[self.kind as usize].fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn observe<S: Stream>(stream: S, kind: Kind) -> impl Stream<Item = S::Item> {
    let counters = counters();
    increment(&counters.active[kind as usize]);
    Observed {
        stream: Box::pin(stream),
        active: Some(Active { counters, kind }),
    }
}
struct Observed<S> {
    stream: Pin<Box<S>>,
    active: Option<Active>,
}
impl<S: Stream> Stream for Observed<S> {
    type Item = S::Item;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> TaskPoll<Option<Self::Item>> {
        let result = self.stream.as_mut().poll_next(cx);
        if matches!(result, TaskPoll::Ready(None)) {
            self.active.take();
        }
        result
    }
}

#[cfg(test)]
tokio::task_local! { static TEST_COUNTERS: Arc<Counters>; }
#[cfg(test)]
pub(crate) async fn isolated_test<F: std::future::Future>(future: F) -> F::Output {
    TEST_COUNTERS.scope(Arc::default(), future).await
}
#[cfg(test)]
pub(crate) fn snapshot(kind: Kind) -> (u64, [u64; 3]) {
    let counters = counters();
    (
        counters.active[kind as usize].load(Ordering::Relaxed),
        std::array::from_fn(|index| counters.polls[kind as usize][index].load(Ordering::Relaxed)),
    )
}
#[cfg(test)]
#[path = "stream_telemetry_tests.rs"]
mod tests;
