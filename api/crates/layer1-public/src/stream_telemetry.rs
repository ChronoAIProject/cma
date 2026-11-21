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

/// The protocol paths covered by semantic close observations in this slice.
#[derive(Clone, Copy, Debug)]
pub enum ContractKind {
    Agui,
    Responses,
    ResponsesV2,
}
impl ContractKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agui => "agui",
            Self::Responses => "responses",
            Self::ResponsesV2 => "responses_v2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CloseReason {
    NormalTerminal = 1,
    FiniteReplay,
    AuthorizationRevoked,
    AuthorizationUnavailable,
    DependencyError,
    ServerError,
    PaginationRequired,
    EofUnexplained,
    DropUnknown,
    UnclassifiedStream,
}
impl CloseReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::NormalTerminal => "normal_terminal",
            Self::FiniteReplay => "finite_replay",
            Self::AuthorizationRevoked => "authorization_revoked",
            Self::AuthorizationUnavailable => "authorization_unavailable",
            Self::DependencyError => "dependency_error",
            Self::ServerError => "server_error",
            Self::PaginationRequired => "pagination_required",
            Self::EofUnexplained => "eof_unexplained",
            Self::DropUnknown => "drop_unknown",
            Self::UnclassifiedStream => "unclassified_stream",
        }
    }
    fn from_byte(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::NormalTerminal,
            2 => Self::FiniteReplay,
            3 => Self::AuthorizationRevoked,
            4 => Self::AuthorizationUnavailable,
            5 => Self::DependencyError,
            6 => Self::ServerError,
            7 => Self::PaginationRequired,
            8 => Self::EofUnexplained,
            9 => Self::DropUnknown,
            10 => Self::UnclassifiedStream,
            _ => return None,
        })
    }
    pub(crate) fn read(error: &layer2_core::sandbox::AgentReadError) -> Self {
        use layer2_core::sandbox::AgentReadError as E;
        match error {
            E::Access(_) | E::Forbidden | E::NotFound => Self::AuthorizationUnavailable,
            E::Unavailable(_) | E::RuntimeCapacity(_) => Self::DependencyError,
            E::TranscriptPaginationRequired => Self::PaginationRequired,
            _ => Self::ServerError,
        }
    }
    pub(crate) fn command(error: &layer2_core::sandbox::agent_io::CommandAdmissionError) -> Self {
        use layer2_core::sandbox::agent_io::{
            CommandAdmissionError as E, CommandAuthorizationError as A, CommandLedgerError as L,
        };
        match error {
            E::Authority(A::Revoked) => Self::AuthorizationRevoked,
            E::Access(_) | E::Authority(A::Unavailable) | E::Ledger(L::NotFound) => {
                Self::AuthorizationUnavailable
            }
            E::Ledger(L::Unavailable) => Self::DependencyError,
            _ => Self::ServerError,
        }
    }
    fn completed(self) -> bool {
        matches!(self, Self::NormalTerminal | Self::FiniteReplay)
    }
}

/// Actual body disposition, supplied only by the existing HTTP body observer.
#[derive(Clone, Copy)]
pub enum BodyEnd {
    Eof,
    Error,
    Dropped,
}

/// Evidence shared through response extensions. Cloning/dropping it never counts
/// a connection; the serving process owns opened/closed counts at the body.
#[derive(Clone)]
pub struct Contract {
    kind: ContractKind,
    evidence: Arc<Evidence>,
}
#[derive(Default)]
struct Evidence {
    reason: std::sync::atomic::AtomicU8,
    terminal_frame: std::sync::atomic::AtomicBool,
}
impl Contract {
    pub fn new(kind: ContractKind) -> Self {
        Self {
            kind,
            evidence: Arc::default(),
        }
    }
    pub fn kind(&self) -> ContractKind {
        self.kind
    }
    pub fn record(&self, reason: CloseReason) {
        // Preserve the first failure even if a later poll reaches natural EOF.
        let _ = self
            .evidence
            .reason
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if CloseReason::from_byte(old).is_some_and(|reason| !reason.completed()) {
                    None
                } else {
                    Some(reason as u8)
                }
            });
    }
    pub(crate) fn terminal_frame(&self) {
        self.evidence.terminal_frame.store(true, Ordering::Relaxed);
    }
    pub(crate) fn replay_complete(&self, finite: bool) {
        self.record(if finite {
            CloseReason::FiniteReplay
        } else if self.evidence.terminal_frame.load(Ordering::Relaxed) {
            CloseReason::NormalTerminal
        } else {
            CloseReason::EofUnexplained
        });
    }
    pub fn close_reason(&self, end: BodyEnd) -> CloseReason {
        let reason = CloseReason::from_byte(self.evidence.reason.load(Ordering::Relaxed));
        if let Some(failure) = reason.filter(|reason| !reason.completed()) {
            return failure;
        }
        match end {
            BodyEnd::Eof => reason.unwrap_or(CloseReason::EofUnexplained),
            BodyEnd::Error => CloseReason::ServerError,
            BodyEnd::Dropped => CloseReason::DropUnknown,
        }
    }
}

/// Called by the serving process's bounded diagnostics timer, never per frame.
pub fn emit() {
    counters().emit();
}
impl Counters {
    fn emit(&self) {
        for (index, kind) in LABELS.iter().enumerate() {
            metrics::gauge!("cma_sse_contract_coverage", "stream_kind" => *kind).set(
                if matches!(*kind, "agui" | "responses" | "responses_v2") {
                    1.0
                } else {
                    0.0
                },
            );
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
