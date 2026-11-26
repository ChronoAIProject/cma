//! Stable Tokio runtime observations, independent of application work.
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use std::time::Duration;
use tokio::{runtime::RuntimeMetrics, task::JoinHandle, time::Instant};

#[cfg(test)]
const INTERVAL: Duration = Duration::from_secs(1);
const INDEXED_WORKERS: usize = 63;
// Two counters per worker label, five scalar series, and ten histograms
// (one wake lag + three pools x three states), each with 16 finite buckets,
// +Inf, count and sum: at most 2 * min(workers, 64) + 195 series (#1805).
const LATENCY_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2., 5., 10., 30., 120., 600., 1800., 3600.,
];

pub(super) fn configure(mut builder: PrometheusBuilder) -> Result<PrometheusBuilder, String> {
    for name in [
        "cma_runtime_wake_lag_seconds",
        "cma_mongo_checkout_wait_seconds",
    ] {
        builder = builder
            .set_buckets_for_metric(Matcher::Full(name.into()), LATENCY_BUCKETS)
            .map_err(|error| error.to_string())?;
    }
    Ok(builder)
}

pub(super) trait RuntimeProbe {
    fn snapshot(&self) -> RuntimeSnapshot;
}

#[derive(Clone, Default)]
pub(super) struct RuntimeSnapshot {
    pub(super) workers: Vec<WorkerSnapshot>,
    pub(super) alive_tasks: usize,
    pub(super) global_queue_depth: usize,
}

#[derive(Clone, Default)]
pub(super) struct WorkerSnapshot {
    pub(super) busy: Duration,
    pub(super) parks: u64,
}

struct TokioProbe(RuntimeMetrics);

impl RuntimeProbe for TokioProbe {
    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            workers: (0..self.0.num_workers())
                .map(|index| WorkerSnapshot {
                    busy: self.0.worker_total_busy_duration(index),
                    parks: self.0.worker_park_count(index),
                })
                .collect(),
            alive_tasks: self.0.num_alive_tasks(),
            global_queue_depth: self.0.global_queue_depth(),
        }
    }
}

pub(super) fn initialize() {
    for (name, description) in [
        ("cma_runtime_workers", "Runtime worker threads"),
        (
            "cma_runtime_alive_tasks",
            "Alive runtime tasks, including waiting tasks",
        ),
        (
            "cma_runtime_global_queue_depth",
            "Injection queue only; excludes worker local queues",
        ),
        (
            "cma_runtime_busy_workers",
            "Workers busy for at least 95% of the last observed interval",
        ),
    ] {
        metrics::describe_gauge!(name, description);
    }
    metrics::describe_counter!(
        "cma_runtime_worker_busy_seconds_total",
        "Cumulative worker busy wall time, whole seconds; not CPU time"
    );
    metrics::describe_counter!("cma_runtime_worker_parks_total", "Cumulative worker parks");
    metrics::describe_counter!(
        "cma_runtime_saturated_seconds_total",
        "Observed seconds with all workers at least 95% busy and no parks"
    );
    metrics::describe_histogram!(
        "cma_runtime_wake_lag_seconds",
        "Runtime sampler delay after its scheduled wake"
    );
    metrics::describe_histogram!(
        "cma_mongo_checkout_wait_seconds",
        "Successful checkout wait, classified by tracked server-pool occupancy at completion"
    );
}

pub(super) fn spawn() -> JoinHandle<()> {
    let probe = TokioProbe(tokio::runtime::Handle::current().metrics());
    tokio::spawn(Sampler::new(probe).run())
}

struct Sampler<P> {
    probe: P,
    previous: Option<(Instant, RuntimeSnapshot)>,
    saturated: Duration,
}

impl<P: RuntimeProbe> Sampler<P> {
    fn new(probe: P) -> Self {
        Self {
            probe,
            previous: None,
            saturated: Duration::ZERO,
        }
    }

    async fn run(self) {
        self.run_with_diagnostics(crate::readiness_diagnostics::configured())
            .await;
    }

    async fn run_with_diagnostics(
        mut self,
        recorder: Option<std::sync::Arc<crate::readiness_diagnostics::Recorder>>,
    ) {
        self.sample(Instant::now());
        crate::readiness_diagnostics::timer::run(recorder, |now, lag| {
            metrics::histogram!("cma_runtime_wake_lag_seconds").record(lag.as_secs_f64());
            self.sample(now);
        })
        .await;
    }

    fn sample(&mut self, now: Instant) {
        let snapshot = self.probe.snapshot();
        let mut busy_workers = 0;
        if let Some((previous_at, previous)) = &self.previous {
            let elapsed = now.saturating_duration_since(*previous_at);
            if !elapsed.is_zero() && previous.workers.len() == snapshot.workers.len() {
                let mut no_parks = true;
                for (worker, before) in snapshot.workers.iter().zip(&previous.workers) {
                    let busy = worker
                        .busy
                        .checked_sub(before.busy)
                        .is_some_and(|delta| delta.as_nanos() * 100 >= elapsed.as_nanos() * 95);
                    busy_workers += usize::from(busy);
                    no_parks &= worker.parks == before.parks;
                }
                if !snapshot.workers.is_empty()
                    && busy_workers == snapshot.workers.len()
                    && no_parks
                {
                    self.saturated = self.saturated.saturating_add(elapsed);
                }
            }
        }
        metrics::gauge!("cma_runtime_workers").set(snapshot.workers.len() as f64);
        metrics::gauge!("cma_runtime_alive_tasks").set(snapshot.alive_tasks as f64);
        metrics::gauge!("cma_runtime_global_queue_depth").set(snapshot.global_queue_depth as f64);
        metrics::gauge!("cma_runtime_busy_workers").set(busy_workers as f64);
        metrics::counter!("cma_runtime_saturated_seconds_total").absolute(self.saturated.as_secs());
        let mut overflow = WorkerSnapshot::default();
        for (index, worker) in snapshot.workers.iter().enumerate() {
            if index < INDEXED_WORKERS {
                export_worker(index.to_string(), worker);
            } else {
                overflow.busy = overflow.busy.saturating_add(worker.busy);
                overflow.parks = overflow.parks.saturating_add(worker.parks);
            }
        }
        if snapshot.workers.len() > INDEXED_WORKERS {
            export_worker("overflow".into(), &overflow);
        }
        self.previous = Some((now, snapshot));
    }
}

fn export_worker(label: String, worker: &WorkerSnapshot) {
    // The facade accepts integer counters. Truncate the cumulative duration,
    // never each interval, so subsecond work is retained across samples.
    metrics::counter!("cma_runtime_worker_busy_seconds_total", "worker" => label.clone())
        .absolute(worker.busy.as_secs());
    metrics::counter!("cma_runtime_worker_parks_total", "worker" => label).absolute(worker.parks);
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
