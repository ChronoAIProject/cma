use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
};

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};

use crate::{
    config::Config,
    ledger::{Ledger, Metrics},
    model::{now, Job},
};

#[derive(Clone)]
pub struct Monitor {
    pub config: Arc<Config>,
    pub ledger: Ledger,
    pub heartbeat: Arc<AtomicI64>,
    pub evidence: Arc<AtomicBool>,
    pub scheduler_ready: Arc<AtomicBool>,
    pub cleanup_failure: Arc<AtomicI64>,
}

impl Monitor {
    pub fn routes(self) -> Router {
        Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .route("/readyz", get(ready))
            .route("/metrics", get(metrics))
            .with_state(self)
    }
}

async fn ready(State(monitor): State<Monitor>) -> StatusCode {
    let last = monitor.heartbeat.load(Ordering::Relaxed);
    let scheduler_ready = monitor.scheduler_ready.load(Ordering::Acquire);
    let available = tokio::task::spawn_blocking(move || monitor.ledger.available().is_ok())
        .await
        .unwrap_or(false);
    if scheduler_ready && available && last > 0 && now() - last <= 10 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn metrics(State(monitor): State<Monitor>) -> impl IntoResponse {
    let ledger = monitor.ledger.clone();
    let result = tokio::task::spawn_blocking(move || ledger.metrics())
        .await
        .ok()
        .and_then(Result::ok);
    let text = render(&monitor, result.as_ref());
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        text,
    )
}

pub fn render(monitor: &Monitor, snapshot: Option<&Metrics>) -> String {
    let mut out = String::new();
    let label = format!("location=\"{}\"", monitor.config.location);
    sample(
        &mut out,
        "state_available",
        &label,
        i64::from(snapshot.is_some()),
    );
    sample(
        &mut out,
        "evidence_available",
        &label,
        i64::from(snapshot.is_some() && monitor.evidence.load(Ordering::Relaxed)),
    );
    sample(
        &mut out,
        "scheduler_heartbeat_timestamp_seconds",
        &label,
        monitor.heartbeat.load(Ordering::Relaxed),
    );
    sample(
        &mut out,
        "cleanup_last_failure_timestamp_seconds",
        &label,
        monitor.cleanup_failure.load(Ordering::Relaxed),
    );
    let jobs = monitor.config.jobs();
    let Some(snapshot) = snapshot else {
        for (job, enabled) in jobs {
            sample(
                &mut out,
                "check_enabled",
                &labels(&label, job),
                i64::from(enabled),
            );
        }
        return out;
    };
    let _ = writeln!(out, "# TYPE cma_journey_phase_duration_seconds histogram");
    for (job, enabled) in jobs {
        let label = labels(&label, job);
        let key = job.key();
        let row = snapshot.jobs.iter().find(|row| row.key == key);
        sample(&mut out, "check_enabled", &label, i64::from(enabled));
        if let Some(row) = row {
            if let Some(since) = row.enabled_since {
                sample(
                    &mut out,
                    "check_enabled_since_timestamp_seconds",
                    &label,
                    since,
                );
            }
            for (name, at) in [
                "scheduled",
                "attempt",
                "completed",
                "success",
                "receipt",
                "terminal",
            ]
            .into_iter()
            .zip(row.times)
            {
                if let Some(at) = at {
                    sample(
                        &mut out,
                        &format!("last_{name}_timestamp_seconds"),
                        &label,
                        at,
                    );
                }
            }
            sample(&mut out, "in_flight", &label, row.active);
            if let Some(oldest) = row.oldest {
                sample(
                    &mut out,
                    "oldest_in_flight_timestamp_seconds",
                    &label,
                    oldest,
                );
            }
        }
        for (_, phase, reason, count) in snapshot.counts.iter().filter(|row| row.0 == key) {
            let (metric, outcome) = match phase.as_str() {
                "first" => (
                    "slots_total",
                    match reason.as_str() {
                        "passed" => "passed",
                        "missed" => "missed",
                        "budget_exhausted" | "target_unsettled" => "skipped",
                        _ => "failed",
                    },
                ),
                "provisional" => ("recovery_placeholders_total", "unpublished"),
                "reconciliation" => (
                    "reconciliations_total",
                    if reason == "passed" {
                        "settled"
                    } else {
                        "failed"
                    },
                ),
                _ => continue,
            };
            sample(
                &mut out,
                metric,
                &format!("{label},outcome=\"{outcome}\",reason=\"{reason}\""),
                *count,
            );
        }
        for (_, phase, values) in snapshot.duration.iter().filter(|row| row.0 == key) {
            let label = format!("{label},phase=\"{phase}\"");
            for (bound, count) in ["0.1", "0.5", "1", "5", "30", "120", "+Inf"]
                .into_iter()
                .zip(values[2..].iter().copied().chain([values[0]]))
            {
                sample(
                    &mut out,
                    "phase_duration_seconds_bucket",
                    &format!("{label},le=\"{bound}\""),
                    count,
                );
            }
            let _ = writeln!(
                out,
                "cma_journey_phase_duration_seconds_sum{{{label}}} {}",
                values[1] as f64 / 1000.0
            );
            sample(&mut out, "phase_duration_seconds_count", &label, values[0]);
        }
    }
    out
}

fn labels(base: &str, job: Job) -> String {
    format!(
        "{base},consumer=\"{}\",check=\"{}\",mode=\"{}\",transport=\"{}\"",
        job.check.consumer(),
        job.check.label(),
        job.mode.label(),
        job.check.transport()
    )
}

fn sample(out: &mut String, name: &str, labels: &str, value: i64) {
    let _ = writeln!(out, "cma_journey_{name}{{{labels}}} {value}");
}
