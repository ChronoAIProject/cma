//! Process liveness and cached store/local execution capability readiness.

use layer2_core::diagnostic_context::DiagnosticContext;
use std::{sync::Arc, time::Duration};

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use layer2_core::StoreError;
use tokio::{sync::watch, time::Instant};

use crate::{
    readiness_diagnostics::{self, Attempt, Recorder},
    store::MongoStores,
};

// Includes server selection, checkout and ping; below the kubelet's 3 s budget.
const CHECK_DEADLINE: Duration = Duration::from_secs(2);
const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CHECK_AGE: Duration = Duration::from_secs(5);

#[async_trait::async_trait]
pub trait ReadinessProbe: Send + Sync {
    /// Must release its resources when dropped. This is a read-only ping, not
    /// an arbitrary Mongo transaction: cancellation of a write is ambiguous.
    async fn ping(&self) -> Result<(), StoreError>;
}

#[async_trait::async_trait]
impl ReadinessProbe for MongoStores {
    async fn ping(&self) -> Result<(), StoreError> {
        // The locked driver's PooledConnection::drop returns the connection;
        // PoolWorker::check_in closes it if a command was interrupted. Keep
        // the real-driver checkout/command cancellation regressions with this.
        MongoStores::ping(self).await
    }
}

#[derive(Clone)]
struct CompletedCheck {
    completed: Instant,
    checked_at: String,
    duration_ms: u128,
    failure: Option<&'static str>,
    diagnostic_check: u64,
}

type CheckState = watch::Receiver<Option<CompletedCheck>>;

#[derive(Clone)]
pub(crate) struct HealthState {
    store: CheckState,
    workers: Option<crate::worker_supervision::Workers>,
    diagnostics: Option<Arc<Recorder>>,
    ownership: Option<std::sync::Weak<layer2_core::agent_ownership::OwnershipCoordinator>>,
}

impl HealthState {
    // Independent application prerequisites only. Ownership admission consumes
    // these checks; including membership here would create a startup cycle.
    pub(crate) fn ready(&self) -> bool {
        self.store.borrow().as_ref().is_some_and(|check| {
            check.failure.is_none() && check.completed.elapsed() < MAX_CHECK_AGE
        }) && self
            .workers
            .as_ref()
            .is_none_or(|workers| workers.snapshot().iter().all(|worker| worker.available()))
    }
}

#[cfg(test)]
pub fn router(probe: Arc<dyn ReadinessProbe>) -> Router {
    router_with_diagnostics(probe, readiness_diagnostics::configured())
}

#[cfg(test)]
pub(crate) fn router_with_workers(
    probe: Arc<dyn ReadinessProbe>,
    workers: crate::worker_supervision::Workers,
) -> Router {
    router_with_readiness(probe, workers, None).0
}

pub(crate) fn router_with_readiness(
    probe: Arc<dyn ReadinessProbe>,
    workers: crate::worker_supervision::Workers,
    ownership: Option<&Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
) -> (Router, HealthState) {
    let mut state = monitored_state(probe, Some(workers), readiness_diagnostics::configured());
    state.ownership = ownership.map(Arc::downgrade);
    (health_routes(state.clone()), state)
}

#[cfg(test)]
pub(crate) fn ownership_fixture(
    probe: Arc<dyn ReadinessProbe>,
    coordinator: &Arc<layer2_core::agent_ownership::OwnershipCoordinator>,
    workers: Option<crate::worker_supervision::Workers>,
) -> (Router, HealthState) {
    let mut state = monitored_state(probe, workers, None);
    state.ownership = Some(Arc::downgrade(coordinator));
    (health_routes(state.clone()), state)
}

#[cfg(test)]
fn router_with_diagnostics(
    probe: Arc<dyn ReadinessProbe>,
    diagnostics: Option<Arc<Recorder>>,
) -> Router {
    monitored(probe, None, diagnostics)
}

#[cfg(test)]
fn monitored(
    probe: Arc<dyn ReadinessProbe>,
    workers: Option<crate::worker_supervision::Workers>,
    diagnostics: Option<Arc<Recorder>>,
) -> Router {
    health_routes(monitored_state(probe, workers, diagnostics))
}

fn monitored_state(
    probe: Arc<dyn ReadinessProbe>,
    workers: Option<crate::worker_supervision::Workers>,
    diagnostics: Option<Arc<Recorder>>,
) -> HealthState {
    let (result, state) = watch::channel(None);
    // The sender owns no receiver. Dropping the last serving state cancels
    // the worker, including during checkout or an unanswered ping.
    tokio::spawn(
        DiagnosticContext::background("health_monitor").scope(monitor(
            probe,
            result,
            diagnostics.clone(),
        )),
    );
    HealthState {
        store: state,
        workers,
        diagnostics,
        ownership: None,
    }
}

#[cfg(test)]
fn routes(state: CheckState, diagnostics: Option<Arc<Recorder>>) -> Router {
    health_routes(HealthState {
        store: state,
        workers: None,
        diagnostics,
        ownership: None,
    })
}

fn health_routes(state: HealthState) -> Router {
    Router::new()
        .route("/livez", get(liveness))
        .route("/ready", get(readiness))
        .with_state(state)
}

async fn monitor(
    probe: Arc<dyn ReadinessProbe>,
    result: watch::Sender<Option<CompletedCheck>>,
    diagnostics: Option<Arc<Recorder>>,
) {
    let mut scheduled = Instant::now();
    tokio::select! {
        biased;
        _ = result.closed() => (),
        _ = async {
            loop {
                let started = Instant::now();
                let mut attempt = Attempt::start(diagnostics.clone(), scheduled, CHECK_DEADLINE);
                let failure = match attempt.check_future(probe.ping()).await {
                    Ok(Ok(())) => None,
                    Ok(Err(_)) => Some("unavailable"),
                    Err(_) => Some("deadline"),
                };
                let completed = Instant::now();
                metrics::gauge!("cma_readiness_check_success").set(if failure.is_none() { 1. } else { 0. });
                metrics::gauge!("cma_readiness_check_timestamp_seconds").set(crate::telemetry::timestamp());
                metrics::histogram!("cma_readiness_check_seconds", "outcome" => failure.unwrap_or("ready"))
                    .record(completed.duration_since(started).as_secs_f64());
                let check = CompletedCheck {
                    completed,
                    checked_at: layer2_core::rfc3339_at(std::time::SystemTime::now()),
                    duration_ms: completed.duration_since(started).as_millis(),
                    failure,
                    diagnostic_check: attempt.check(),
                };
                let changed = result.borrow().as_ref()
                    .is_none_or(|previous| previous.failure != failure);
                if changed && diagnostics.is_none() {
                    tracing::info!(
                        serving_capacity = failure.is_none(),
                        duration_ms = check.duration_ms as u64,
                        reason = failure.unwrap_or("ready"),
                        "readiness dependency state changed"
                    );
                }
                if diagnostics.is_none() { tracing::debug!(
                    duration_ms = check.duration_ms as u64,
                    reason = failure.unwrap_or("ready"),
                    "readiness dependency check completed"
                ); }
                result.send_replace(Some(check));
                attempt.finish(failure.unwrap_or("ready"));
                if changed {
                    if let Some(diagnostics) = &diagnostics {
                        diagnostics.state_changed(failure.unwrap_or("ready"), completed.duration_since(started).as_millis() as u64);
                    }
                }
                // Delay after completion: no overlapping attempts or catch-up
                // ticks, regardless of the number of incoming health requests.
                scheduled = Instant::now() + CHECK_INTERVAL;
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        } => (),
    }
}

async fn liveness() -> &'static str {
    "ok"
}

async fn readiness(State(state): State<HealthState>) -> impl IntoResponse {
    let check = state.store.borrow().clone();
    let age = check.as_ref().map(|check| check.completed.elapsed());
    let failure = match &check {
        None => Some("pending"),
        Some(_) if age.is_some_and(|age| age >= MAX_CHECK_AGE) => Some("stale"),
        Some(check) => check.failure,
    };
    let workers = state
        .workers
        .as_ref()
        .map(|workers| workers.snapshot())
        .unwrap_or_default();
    let execution_available = workers.iter().all(|worker| worker.available());
    let ownership_ready = state.ownership.as_ref().is_none_or(|ownership| {
        ownership
            .upgrade()
            .is_some_and(|coordinator| coordinator.can_acquire())
    });
    let ready = failure.is_none() && execution_available && ownership_ready;
    if let Some(diagnostics) = &state.diagnostics {
        // Response evidence must include worker refusal even after a healthy ping.
        // Keep a dependency failure's more specific phase reason when both fail.
        diagnostics.response(
            check.as_ref().map_or(0, |c| c.diagnostic_check),
            failure.unwrap_or(if !execution_available {
                "workers"
            } else if !ownership_ready {
                "ownership_admission"
            } else {
                "ready"
            }),
            age,
            check.as_ref().map_or(0, |c| {
                c.duration_ms.saturating_mul(1000).min(u128::from(u64::MAX)) as u64
            }),
        );
    }
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(serde_json::json!({
            "status": if ready { "ready" } else { "not-ready" },
            "reason": if failure.is_some() { Some("store") } else if !execution_available { Some("workers") } else if !ownership_ready { Some("ownership_admission") } else { None },
            "workers": workers,
            "dependency": {
                "checked_at": check.as_ref().map(|check| &check.checked_at),
                "age_ms": age.map(|age| age.as_millis()),
                "duration_ms": check.as_ref().map(|check| check.duration_ms),
                "reason": failure,
            },
        })),
    )
}

#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod gateway_tests;
#[cfg(test)]
mod mongo_fixture;
#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod diagnostic_tests;

#[cfg(test)]
mod ownership_tests;
