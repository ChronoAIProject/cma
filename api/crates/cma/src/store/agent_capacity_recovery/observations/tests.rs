use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn discovery_metrics_keep_failure_and_debug_activity_out_of_caller_logs() {
    for json in [false, true] {
        let output = Output::default();
        let capture = output.clone();
        let builder = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || capture.clone());
        let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = if json {
            Box::new(builder.json().finish())
        } else {
            Box::new(builder.finish())
        };
        let _tracing = tracing::subscriber::set_default(subscriber);
        let caller = tracing::info_span!("caller", private = "synthetic-private-discovery-caller");
        let _entered = caller.enter();
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let command: mongodb::error::CommandError = mongodb::bson::deserialize_from_document(
            mongodb::bson::doc! {"code": 2, "codeName": "PRIVATE_CODE_NAME",
            "errmsg": "PRIVATE_PAYLOAD owner=PRIVATE_ID"},
        )
        .unwrap();
        metrics::with_local_recorder(&recorder, || {
            let mut failed = Observation::new(Phase::AdmittedQuery);
            assert_eq!(
                failed.driver(&ErrorKind::Command(command).into()),
                RecoveryError::Unavailable
            );
            let mut completed = Observation::new(Phase::ExpiredQuery);
            completed.complete();
        });
        tracing::debug!("ordinary discovery control");
        let encoded = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(encoded.lines().count(), 1, "{encoded}");
        assert!(encoded.contains("ordinary discovery control"));
        assert!(encoded.contains("synthetic-private-discovery-caller"));
        assert!(!encoded.contains("PRIVATE_PAYLOAD"));
        let metrics = handle.render();
        for (phase, outcome) in [("admitted_query", "driver"), ("expired_query", "success")] {
            assert!(metrics.lines().any(|line| {
                line.starts_with("cma_capacity_recovery_discovery_total{")
                    && line.contains(&format!("phase=\"{phase}\""))
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(" 1")
            }));
        }
        assert!(!metrics.contains("PRIVATE_"), "{metrics}");
        assert!(
            !metrics.contains("synthetic-private-discovery-caller"),
            "{metrics}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_discovery_future_records_exactly_one_terminal_outcome() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let result = tokio::time::timeout(std::time::Duration::from_millis(1), async {
        let _observed = Observation::new(Phase::ExpiredQuery);
        std::future::pending::<()>().await;
    })
    .await;
    assert!(result.is_err());
    let metrics = handle.render();
    let samples: Vec<_> = metrics
        .lines()
        .filter(|line| line.starts_with("cma_capacity_recovery_discovery_total{"))
        .collect();
    assert_eq!(samples.len(), 1, "{metrics}");
    assert!(samples[0].contains("phase=\"expired_query\""));
    assert!(samples[0].contains("outcome=\"cancelled\""));
    assert!(samples[0].ends_with(" 1"));
    assert!(metrics
        .contains("cma_capacity_recovery_discovery_seconds_count{phase=\"expired_query\"} 1"));
}

#[test]
fn cohort_labels_distinguish_refusals_and_replays_from_commits() {
    use CapacityRecoveryDisposition as D;
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        for disposition in [D::ContinueOriginalWork, D::CleanupOnly, D::LeaseActive] {
            preflight(disposition);
        }
        commit(Stage::Admitted, &Ok(CommitOutcome::Recorded(())));
        commit(Stage::Completed, &Ok(CommitOutcome::Recorded(())));
        cohort(Stage::Completed, Outcome::Replay);
        commit::<()>(Stage::Completed, &Ok(CommitOutcome::Unknown));
        commit::<()>(Stage::Admitted, &Ok(CommitOutcome::KnownAbort));
        commit::<()>(Stage::Admitted, &Err(RecoveryError::Unauthorized));
    });
    let rendered = handle.render();
    let samples: Vec<_> = rendered
        .lines()
        .filter(|line| line.starts_with("cma_capacity_recovery_cohort_total{"))
        .collect();
    for (stage, outcome, count) in [
        ("scanned", "observed", 3),
        ("eligible", "observed", 1),
        ("skipped", "observed", 1),
        ("blocked", "refused", 1),
        ("admitted", "committed", 1),
        ("completed", "committed", 1),
        ("completed", "replay", 1),
        ("blocked", "unknown", 1),
        ("blocked", "known_abort", 1),
    ] {
        assert!(
            samples.iter().any(|line| {
                line.contains(&format!("stage=\"{stage}\""))
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(&format!(" {count}"))
                    && line.matches('=').count() == 2
            }),
            "{rendered}"
        );
    }
    assert_eq!(samples.len(), 9, "{rendered}");
}
