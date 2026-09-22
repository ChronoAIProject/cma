//! Readiness gates local startup; CMA reachability is diagnostic only.
use crate::records::RecordClient;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

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
            }),
        }
    }
    pub fn start_draining(&self) {
        self.inner.draining.store(true, Ordering::Release);
    }
    pub fn report(&self) -> ReadinessReport {
        let mut reasons = Vec::new();
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
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let mut response = (status, Json(report.clone())).into_response();
    // Readiness has a typed diagnostic body even when local startup is incomplete.
    response.extensions_mut().insert(report);
    response
}
