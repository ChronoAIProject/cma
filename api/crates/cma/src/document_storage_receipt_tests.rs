use super::failure_tests::{authorize, client, key, operation};
use super::*;
use crate::dependency_test_support::{finish, Peer};
use serde_json::json;

#[derive(Clone, Default)]
struct LogOutput(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogOutput {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn receipt_mismatch_increments_metric_without_logging_key_or_source() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let logs = LogOutput::default();
    // Match production's text formatter, which honors an event's explicit root.
    // The JSON formatter falls back to ambient spans even for root events.
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::WARN)
        .with_writer(logs.clone())
        .finish();
    let _tracing = tracing::subscriber::set_default(subscriber);
    let outer = tracing::warn_span!("request", source = "private-source", key = %key());
    let _entered = outer.enter();
    let mut peer = Peer::start().await;
    let client = client(&peer);
    for (index, field) in ["key", "length", "generation", "digest", "fence"]
        .into_iter()
        .enumerate()
    {
        let task = operation(&client, true);
        if index == 0 {
            authorize(&mut peer).await;
        }
        let mut receipt = json!({"key":key(),"contentLength":7,"generation":1,
            "digest":digest(b"fixture"),"fenceVersion":1});
        match field {
            "key" => receipt["key"] = json!("private-receipt-key"),
            "length" => receipt["contentLength"] = json!(8),
            "generation" => receipt["generation"] = json!(2),
            "digest" => receipt["digest"] = json!("private-receipt-digest"),
            "fence" => receipt["fenceVersion"] = json!(2),
            _ => unreachable!(),
        }
        peer.next().await.json(json!({"data":receipt}));
        assert_eq!(finish(task).await, Err(DocumentError::Corrupt));
        assert!(handle.render().contains(&format!(
            "cma_document_storage_receipt_mismatch_total{{field=\"{field}\"}} 1"
        )));
    }
    let task = operation(&client, true);
    peer.next()
        .await
        .json(json!({"data":{"key":key(),"contentLength":7,"generation":1,
        "digest":digest(b"fixture"),"fenceVersion":1}}));
    assert_eq!(finish(task).await, Ok(vec![]));
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 5, "{logs}");
    for (line, field) in logs
        .lines()
        .zip(["key", "length", "generation", "digest", "fence"])
    {
        assert_eq!(
            line.trim(),
            format!(
                "WARN cma::document_storage: document storage receipt mismatch field=\"{field}\""
            )
        );
    }
    for field in ["key", "length", "generation", "digest", "fence"] {
        assert!(handle.render().contains(&format!(
            "cma_document_storage_receipt_mismatch_total{{field=\"{field}\"}} 1"
        )));
    }
    for private in [
        key(),
        "fixture".into(),
        "private".into(),
        "local-fixture".into(),
        digest(b"fixture"),
    ] {
        assert!(!logs.contains(&private), "{logs}");
    }
}
