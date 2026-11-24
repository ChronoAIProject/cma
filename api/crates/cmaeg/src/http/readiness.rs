//! Readiness gates local startup; CMA reachability is diagnostic only.
use crate::records::RecordClient;
use axum::{extract::State, response::Response};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Clone, Default)]
pub struct Readiness {
    inner: Arc<Inner>,
}
#[derive(Default)]
struct Inner {
    keys_loaded: AtomicBool,
    registry_loaded: AtomicBool,
    cma_reachable: AtomicBool,
    draining: AtomicBool,
    routed_recovery_failed: AtomicBool,
    replica_required: AtomicBool,
    replica_joined: AtomicBool,
    /// Set once `/readyz` first answered ready; background work that must
    /// never compete with startup waits for it.
    served_ready: AtomicBool,
    served: Notify,
}

pub use cmaeg_engine::ReadinessReport;

impl Readiness {
    pub fn initialized() -> Self {
        Self {
            inner: Arc::new(Inner {
                keys_loaded: true.into(),
                registry_loaded: true.into(),
                cma_reachable: false.into(),
                draining: false.into(),
                routed_recovery_failed: false.into(),
                replica_required: false.into(),
                replica_joined: false.into(),
                served_ready: false.into(),
                served: Notify::new(),
            }),
        }
    }
    /// Note that `/readyz` answered ready, waking [`Readiness::served`].
    pub fn note_served_ready(&self) {
        if !self.inner.served_ready.swap(true, Ordering::AcqRel) {
            self.inner.served.notify_waiters();
        }
    }
    /// Wait until `/readyz` has answered ready at least once.
    pub async fn served(&self) {
        loop {
            // Register before checking, so a concurrent note cannot be lost.
            let notified = self.inner.served.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.served_ready.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
    pub fn start_draining(&self) {
        self.inner.draining.store(true, Ordering::Release);
    }
    pub(crate) fn require_replica_membership(&self) {
        self.inner.replica_required.store(true, Ordering::Release);
    }
    pub(crate) fn replica_membership(&self, joined: bool) {
        self.inner.replica_joined.store(joined, Ordering::Release);
    }
    pub fn routed_recovery_failed(&self) {
        self.inner
            .routed_recovery_failed
            .store(true, Ordering::Release);
    }
    pub fn report(&self) -> ReadinessReport {
        let mut reasons = Vec::new();
        if self.inner.replica_required.load(Ordering::Acquire)
            && !self.inner.replica_joined.load(Ordering::Acquire)
        {
            reasons.push("replica_membership_unavailable");
        }
        if self.inner.routed_recovery_failed.load(Ordering::Acquire) {
            reasons.push("routed_recovery_failed");
        }
        if self.inner.draining.load(Ordering::Acquire) {
            reasons.push("draining");
        }
        if !self.inner.keys_loaded.load(Ordering::Acquire) {
            reasons.push("master_keys_not_loaded");
        }
        if !self.inner.registry_loaded.load(Ordering::Acquire) {
            reasons.push("provider_registry_not_parsed");
        }
        ReadinessReport {
            ready: reasons.is_empty(),
            reasons,
            cma_reachable: self.inner.cma_reachable.load(Ordering::Acquire),
        }
    }
    pub async fn probe_once(&self, records: &RecordClient) {
        self.inner
            .cma_reachable
            .store(records.reachable().await, Ordering::Release);
    }
    pub fn start_probe(&self, records: Arc<RecordClient>) -> ReachabilityProbe {
        let readiness = self.clone();
        ReachabilityProbe(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                readiness.probe_once(&records).await;
            }
        }))
    }
}
/// Own the probe with the server lifetime; shutdown never leaves a background writer.
pub struct ReachabilityProbe(tokio::task::JoinHandle<()>);
impl Drop for ReachabilityProbe {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn readyz(State(readiness): State<Readiness>) -> Response {
    let report = readiness.report();
    if report.ready {
        readiness.note_served_ready();
    }
    cmaeg_http::http::readiness::readiness_response(
        report,
        option_env!("CHRONO_BUILD_SHA"),
        option_env!("CHRONO_BUILD"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn served_waits_for_the_first_ready_answer() {
        let readiness = Readiness::initialized();
        let waiting = tokio::spawn({
            let readiness = readiness.clone();
            async move { readiness.served().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        assert_eq!(readyz(State(readiness.clone())).await.status(), 200);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        // Later waiters return at once.
        readiness.served().await;
    }

    #[tokio::test]
    async fn an_unready_answer_is_not_noted() {
        let readiness = Readiness::initialized();
        readiness.start_draining();
        assert_eq!(readyz(State(readiness.clone())).await.status(), 503);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), readiness.served())
                .await
                .is_err()
        );
    }
}
