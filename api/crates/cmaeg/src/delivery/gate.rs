//! The admission hook every send path calls before its first side effect.
//!
//! A gate runs a send's route admission, its commit guard and a limit per
//! HTTP attempt (Amendment R3 §R3.1). The legacy lane passes [`Ungated`],
//! which admits everything at once and always commits, so today's side
//! effects keep their order; routed delivery supplies a gate that applies
//! its stages.
use super::intents::DeliveryOutcome;
use cmaeg_core::channel::Secret;
use futures_util::future::{ready, BoxFuture};

/// The route a send enters when it addresses no chat, as a custom event's
/// result webhooks do, so a routed gate applies only its record and owner
/// stages. Every chat route id is non-empty.
pub const NO_CHAT_ROUTE: &str = "";

pub trait SendGate: Send + Sync {
    /// Admission stages 1–4 for `route`, held for the whole send.
    fn enter_route<'a>(&'a self, route: &'a str)
        -> BoxFuture<'a, Result<RouteTicket, GateRefusal>>;
    /// Stage 7, the commit guard, called immediately before a send's first
    /// side effect. Idempotent: after the first `true`, later calls return
    /// `true` and change nothing.
    fn commit(&self) -> bool;
    /// Stage 6, taken before each HTTP attempt and dropped before any backoff.
    fn attempt<'a>(&'a self) -> BoxFuture<'a, Result<AttemptPermit, GateRefusal>>;
    /// Stamps the send's slot as making progress.
    fn progress(&self);

    /// Revalidate a result webhook after DNS resolution and before its POST.
    fn authorize_webhook<'a>(&'a self, _url: &'a str, _secret: &'a Secret) -> BoxFuture<'a, bool> {
        Box::pin(ready(true))
    }

    /// Deadline for transport and pacing, when the caller supplies one.
    fn transport_deadline(&self) -> Option<tokio::time::Instant> {
        None
    }
}

pub(super) async fn within_deadline<T>(
    gate: &dyn SendGate,
    work: impl std::future::Future<Output = T>,
    on_timeout: T,
) -> T {
    match gate.transport_deadline() {
        Some(deadline) => tokio::time::timeout_at(deadline, work)
            .await
            .unwrap_or(on_timeout),
        None => work.await,
    }
}

/// Why a gate refused a send. The send ends with this refusal's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateRefusal {
    pub reason: &'static str,
    pub retry_after: Option<u64>,
}
impl GateRefusal {
    /// Rate limited when the gate names a retry, rejected otherwise.
    pub fn outcome(&self) -> DeliveryOutcome {
        match self.retry_after {
            Some(retry_after) => DeliveryOutcome::RateLimited { retry_after },
            None => DeliveryOutcome::Rejected {
                reason: self.reason.into(),
            },
        }
    }
}

/// The outcome of a send whose commit guard refused it: it was never sent.
pub(super) fn expired() -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: "delivery_expired".into(),
    }
}

/// What a ticket or permit releases when dropped; `None` for nothing.
type Held = Option<Box<dyn Send + Sync>>;

/// A route admission. Dropping it releases what the gate took.
#[must_use = "dropping a ticket releases its route admission"]
pub struct RouteTicket {
    _held: Held,
}
impl RouteTicket {
    /// A ticket that holds nothing.
    pub fn empty() -> Self {
        Self { _held: None }
    }
    /// A ticket that releases `held` when it is dropped.
    pub fn holding(held: impl Send + Sync + 'static) -> Self {
        Self {
            _held: Some(Box::new(held)),
        }
    }
}

/// Admission for one HTTP attempt. Dropping it releases what the gate took.
#[must_use = "dropping a permit releases its attempt admission"]
pub struct AttemptPermit {
    _held: Held,
}
impl AttemptPermit {
    /// A permit that holds nothing.
    pub fn empty() -> Self {
        Self { _held: None }
    }
    /// A permit that releases `held` when it is dropped.
    pub fn holding(held: impl Send + Sync + 'static) -> Self {
        Self {
            _held: Some(Box::new(held)),
        }
    }
}

/// Today's lane: every route and attempt is admitted at once from an
/// already-ready future, and every send commits.
pub struct Ungated;
impl SendGate for Ungated {
    fn enter_route<'a>(
        &'a self,
        _route: &'a str,
    ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(ready(Ok(RouteTicket::empty())))
    }
    fn commit(&self) -> bool {
        true
    }
    fn attempt<'a>(&'a self) -> BoxFuture<'a, Result<AttemptPermit, GateRefusal>> {
        Box::pin(ready(Ok(AttemptPermit::empty())))
    }
    fn progress(&self) {}
}

/// A gate for unit tests that logs every call, commits as told, and checks
/// that no attempt permit outlives its attempt.
#[cfg(test)]
pub(crate) mod recording {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::time::Instant;

    pub(crate) struct RecordingGate {
        /// Each call with the whole seconds elapsed since the gate was made.
        calls: Mutex<Vec<(&'static str, u64)>>,
        /// Every live attempt permit holds a clone.
        live: Arc<()>,
        commits: bool,
        started: Instant,
    }
    impl RecordingGate {
        pub(crate) fn new(commits: bool) -> Self {
            Self {
                calls: Mutex::default(),
                live: Arc::new(()),
                commits,
                started: Instant::now(),
            }
        }
        pub(crate) fn calls(&self) -> Vec<&'static str> {
            self.timed_calls()
                .into_iter()
                .map(|(call, _)| call)
                .collect()
        }
        pub(crate) fn timed_calls(&self) -> Vec<(&'static str, u64)> {
            self.calls.lock().unwrap().clone()
        }
        fn log(&self, call: &'static str) {
            let second = self.started.elapsed().as_secs();
            self.calls.lock().unwrap().push((call, second));
        }
    }
    impl SendGate for RecordingGate {
        fn enter_route<'a>(
            &'a self,
            _route: &'a str,
        ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
            self.log("enter_route");
            Box::pin(ready(Ok(RouteTicket::empty())))
        }
        fn commit(&self) -> bool {
            self.log("commit");
            self.commits
        }
        fn attempt<'a>(&'a self) -> BoxFuture<'a, Result<AttemptPermit, GateRefusal>> {
            self.log("attempt");
            Box::pin(ready(Ok(AttemptPermit::holding(self.live.clone()))))
        }
        fn progress(&self) {
            assert_eq!(
                Arc::strong_count(&self.live),
                1,
                "an attempt permit outlived its attempt"
            );
            self.log("progress");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn ungated_admits_everything_and_always_commits() {
        let gate = Ungated;
        assert!(gate.enter_route("route-1").await.is_ok());
        assert!(gate.enter_route(NO_CHAT_ROUTE).await.is_ok());
        assert!(gate.attempt().await.is_ok());
        assert!(gate.commit());
        assert!(gate.commit());
        gate.progress();
    }

    #[test]
    fn tickets_and_permits_release_what_they_hold_when_dropped() {
        let held = Arc::new(());
        let ticket = RouteTicket::holding(held.clone());
        let permit = AttemptPermit::holding(held.clone());
        assert_eq!(Arc::strong_count(&held), 3);
        drop(ticket);
        assert_eq!(Arc::strong_count(&held), 2);
        drop(permit);
        assert_eq!(Arc::strong_count(&held), 1);
        drop((RouteTicket::empty(), AttemptPermit::empty()));
    }

    #[test]
    fn refusals_map_to_rate_limits_or_rejections() {
        let congested = GateRefusal {
            reason: "route_congested",
            retry_after: None,
        };
        assert_eq!(
            congested.outcome(),
            DeliveryOutcome::Rejected {
                reason: "route_congested".into()
            }
        );
        let full = GateRefusal {
            reason: "route_capacity",
            retry_after: Some(10),
        };
        assert_eq!(
            full.outcome(),
            DeliveryOutcome::RateLimited { retry_after: 10 }
        );
        assert_eq!(
            expired(),
            DeliveryOutcome::Rejected {
                reason: "delivery_expired".into()
            }
        );
    }
}
