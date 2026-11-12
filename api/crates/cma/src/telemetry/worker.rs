//! Local scheduler liveness and phase health. Success can include an idle scan.
const WORKERS: [&str; 8] = [
    "launches",
    "agent_commands",
    "sandbox_commands",
    "agents",
    "sandboxes",
    "workspaces",
    "sandbox_reconciliation",
    "native_warm_pools",
];

pub(super) fn initialize() {
    for worker in WORKERS {
        metrics::gauge!("cma_worker_alive", "worker" => worker).set(0);
        metrics::gauge!("cma_worker_execution_available", "worker" => worker).set(0);
        metrics::counter!("cma_worker_restarts_total", "worker" => worker).absolute(0);
        metrics::gauge!("cma_worker_heartbeat_timestamp_seconds", "worker" => worker).set(0);
        for phase in ["discovery", "execution", "reconciliation"] {
            metrics::gauge!("cma_worker_phase_success_timestamp_seconds", "worker" => worker, "phase" => phase).set(0);
        }
    }
}

pub(crate) struct Worker(&'static str);
impl Worker {
    pub(crate) fn start(name: &'static str) -> Self {
        metrics::gauge!("cma_worker_alive", "worker" => name).increment(1.);
        let worker = Self(name);
        worker.heartbeat();
        worker
    }
    pub(crate) fn heartbeat(&self) {
        metrics::gauge!("cma_worker_heartbeat_timestamp_seconds", "worker" => self.0)
            .set(super::timestamp());
    }
    pub(crate) fn queue(&self, queued: usize, active: usize) {
        metrics::gauge!("cma_worker_local_queued", "worker" => self.0).set(queued as f64);
        metrics::gauge!("cma_worker_local_active", "worker" => self.0).set(active as f64);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        metrics::gauge!("cma_worker_alive", "worker" => self.0).decrement(1.);
        self.queue(0, 0);
    }
}

pub(crate) fn completed(worker: &'static str, phase: &'static str, outcome: &'static str) {
    metrics::counter!("cma_worker_operations_total", "worker" => worker, "phase" => phase, "outcome" => outcome).increment(1);
    if matches!(outcome, "ok" | "idle" | "attempted") {
        metrics::gauge!("cma_worker_phase_success_timestamp_seconds", "worker" => worker, "phase" => phase).set(super::timestamp());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwinding_worker_clears_liveness_and_preserves_last_heartbeat() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let failure = std::panic::catch_unwind(|| {
                let worker = Worker::start("agent_commands");
                worker.queue(2, 1);
                panic!("scheduler stopped");
            });
            assert!(failure.is_err());
        });
        let text = handle.render();
        assert!(text.contains("cma_worker_alive{worker=\"agent_commands\"} 0"));
        assert!(text.contains("cma_worker_local_queued{worker=\"agent_commands\"} 0"));
        assert!(text.contains("cma_worker_heartbeat_timestamp_seconds{worker=\"agent_commands\"}"));
    }
}

#[cfg(test)]
#[test]
fn idle_scan_is_phase_health_without_claiming_business_progress() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        completed("native_warm_pools", "execution", "idle");
        let before = handle.render();
        completed("native_warm_pools", "execution", "error");
        let success = |text: &str| {
            text.lines()
                .find(|line| line.starts_with("cma_worker_phase_success_timestamp_seconds{"))
                .unwrap()
                .to_owned()
        };
        assert_eq!(success(&before), success(&handle.render()));
        assert!(!handle
            .render()
            .contains("cma_worker_progress_timestamp_seconds"));
    });
}
