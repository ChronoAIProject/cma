//! Readiness gates local startup; CMA reachability is diagnostic only.
use crate::records::RecordClient;
use serde::Serialize;
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

#[derive(Clone, Serialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub reasons: Vec<&'static str>,
    pub cma_reachable: bool,
}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_membership_recovery_and_drain_gates_in_report_order() {
        let readiness = Readiness::initialized();
        let observer = readiness.clone();
        readiness.require_replica_membership();
        assert_eq!(
            observer.report().reasons,
            ["replica_membership_unavailable"]
        );
        readiness.replica_membership(true);
        assert!(observer.report().ready);
        assert!(!observer.report().cma_reachable);
        readiness.replica_membership(false);
        readiness.routed_recovery_failed();
        readiness.start_draining();
        assert!(!observer.report().ready);
        assert_eq!(
            observer.report().reasons,
            [
                "replica_membership_unavailable",
                "routed_recovery_failed",
                "draining"
            ]
        );

        let uninitialized = Readiness::default();
        uninitialized.require_replica_membership();
        uninitialized.routed_recovery_failed();
        uninitialized.start_draining();
        assert_eq!(
            uninitialized.report().reasons,
            [
                "replica_membership_unavailable",
                "routed_recovery_failed",
                "draining",
                "master_keys_not_loaded",
                "provider_registry_not_parsed",
            ]
        );
    }

    #[tokio::test]
    async fn dropping_probe_cancels_an_inflight_read_and_releases_shared_state() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let records = Arc::new(
            RecordClient::new(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture",
            )
            .unwrap(),
        );
        let readiness = Readiness::initialized();
        let probe = readiness.start_probe(records.clone());
        let task = probe.0.abort_handle();
        let _connection = tokio::time::timeout(Duration::from_secs(1), async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
                assert!(request.len() < 4096);
            }
            assert!(request.starts_with(b"GET /ready HTTP/1.1\r\n"));
            stream
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        drop(probe);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(Arc::strong_count(&readiness.inner), 1);
        assert_eq!(Arc::strong_count(&records), 1);
        assert!(readiness.report().ready);
        assert!(!readiness.report().cma_reachable);
    }
}
