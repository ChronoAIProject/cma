//! Process liveness and a single bounded, cached application-store check.

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

pub fn router(probe: Arc<dyn ReadinessProbe>) -> Router {
    router_with_diagnostics(probe, readiness_diagnostics::configured())
}

fn router_with_diagnostics(
    probe: Arc<dyn ReadinessProbe>,
    diagnostics: Option<Arc<Recorder>>,
) -> Router {
    let (result, state) = watch::channel(None);
    // The sender owns no receiver. Dropping the last router cancels the check
    // and stops the worker, including during checkout or an unanswered ping.
    tokio::spawn(
        DiagnosticContext::background("health_monitor").scope(monitor(
            probe,
            result,
            diagnostics.clone(),
        )),
    );
    routes(state, diagnostics)
}

#[derive(Clone)]
struct HealthState {
    checks: CheckState,
    diagnostics: Option<Arc<Recorder>>,
}

fn routes(state: CheckState, diagnostics: Option<Arc<Recorder>>) -> Router {
    Router::new()
        .route("/livez", get(liveness))
        .route("/ready", get(readiness))
        .with_state(HealthState {
            checks: state,
            diagnostics,
        })
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
    let check = state.checks.borrow().clone();
    let age = check.as_ref().map(|check| check.completed.elapsed());
    let failure = match &check {
        None => Some("pending"),
        Some(_) if age.is_some_and(|age| age >= MAX_CHECK_AGE) => Some("stale"),
        Some(check) => check.failure,
    };
    let ready = failure.is_none();
    if let Some(diagnostics) = &state.diagnostics {
        diagnostics.response(
            check.as_ref().map_or(0, |c| c.diagnostic_check),
            failure.unwrap_or("ready"),
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
            "reason": if ready { None } else { Some("store") },
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
mod gateway_tests;
#[cfg(test)]
mod mongo_fixture;
#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod diagnostic_tests;
