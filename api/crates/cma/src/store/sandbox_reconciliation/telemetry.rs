//! Fixed-stage timings survive errors and cancellation of the owning transaction.
use std::{future::Future, time::Instant};

pub(super) struct Phase {
    stage: &'static str,
    started: Instant,
    outcome: &'static str,
    documents: crate::store::mongo_read_telemetry::DocumentObserver,
}

impl Phase {
    pub(super) fn new(stage: &'static str) -> Self {
        Self {
            stage,
            started: Instant::now(),
            outcome: "cancelled",
            documents: crate::store::mongo_read_telemetry::DocumentObserver::new("agent_log"),
        }
    }

    pub(super) fn finish(&mut self, success: bool) {
        self.outcome = if success { "complete" } else { "error" };
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        tracing::info!(
            stage = self.stage,
            outcome = self.outcome,
            elapsed_us = self.started.elapsed().as_micros() as u64,
            documents = self.documents.documents(),
            "runtime loss transaction phase"
        );
    }
}

pub(super) async fn measure<T, E>(
    stage: &'static str,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut phase = Phase::new(stage);
    let result = phase.documents.scope(work).await;
    phase.finish(result.is_ok());
    result
}

pub(super) fn agent_counts(agents: &[layer2_core::sandbox::Agent]) {
    tracing::info!(
        agents = agents.len(),
        live_agents = agents.iter().filter(|agent| agent.status.is_live()).count(),
        "runtime loss transaction agents"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timings_include_success_failure_cancellation_and_partial_consumption() {
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        for success in [true, false] {
            let result = measure("log_bounded", async {
                crate::store::mongo_read_telemetry::Read::find("agent_log").document(10);
                if success {
                    Ok(())
                } else {
                    Err(())
                }
            })
            .await;
            assert_eq!(result.is_ok(), success);
        }
        assert!(tokio::time::timeout(
            std::time::Duration::from_secs(1),
            measure("log_full", async {
                crate::store::mongo_read_telemetry::Read::find("agent_log").document(10);
                std::future::pending::<Result<(), ()>>().await
            })
        )
        .await
        .is_err());
        let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(logs.lines().count(), 3);
        for outcome in ["complete", "error", "cancelled"] {
            assert!(logs
                .lines()
                .any(|line| line.contains(&format!("outcome=\"{outcome}\""))
                    && line.contains("documents=1")));
        }
    }
}
