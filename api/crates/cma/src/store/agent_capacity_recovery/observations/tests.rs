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
fn discovery_driver_diagnostics_exclude_raw_error_text_and_identity() {
    let output = Output::default();
    let capture = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(move || capture.clone())
        .finish();
    let _tracing = tracing::subscriber::set_default(subscriber);
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let command: mongodb::error::CommandError = mongodb::bson::deserialize_from_document(
        mongodb::bson::doc! {"code": 2, "codeName": "PRIVATE_CODE_NAME",
        "errmsg": "PRIVATE_PAYLOAD owner=PRIVATE_ID"},
    )
    .unwrap();
    metrics::with_local_recorder(&recorder, || {
        let mut observed = Observation::new(Phase::AdmittedQuery);
        observed.row();
        assert_eq!(
            observed.driver(&ErrorKind::Command(command).into()),
            RecoveryError::Unavailable
        );
    });
    let output = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let event: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(event["fields"]["phase"], "admitted_query");
    assert_eq!(event["fields"]["reason"], "driver");
    assert_eq!(event["fields"]["driver_code"], 2);
    assert_eq!(event["fields"]["rows_seen"], 1);
    assert_eq!(event["fields"]["transient_transaction"], false);
    let metrics = handle.render();
    assert!(metrics.contains("outcome=\"driver\""));
    for private in ["PRIVATE_", "owner=", "agent_id", "sandbox_id"] {
        assert!(!output.contains(private), "{output}");
        assert!(!metrics.contains(private), "{metrics}");
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
