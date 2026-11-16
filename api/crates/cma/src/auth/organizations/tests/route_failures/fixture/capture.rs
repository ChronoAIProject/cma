use std::sync::{Arc, Mutex};

use layer2_core::diagnostic_context::DiagnosticContext;
use serde_json::{json, Value};
use tracing::instrument::WithSubscriber;
use tracing::Instrument;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) async fn run<F: std::future::Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let context = DiagnosticContext {
        request_hash: "safe-org-request-hash".into(),
        ..DiagnosticContext::default()
    };
    let result = context
        .scope(future.instrument(tracing::info_span!(
            "private-parent",
            secret = "must-not-leak-parent"
        )))
        .with_subscriber(subscriber)
        .await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "organization_authority_failure")
        .collect();
    for event in &events {
        assert_eq!(event["fields"]["request_hash"], "safe-org-request-hash");
        assert_eq!(event["span"], json!({"name":"organization_authority"}));
        assert_eq!(event["spans"], json!([{"name":"organization_authority"}]));
        assert!(event["fields"]["elapsed_ms"].is_u64());
        for private in [
            "must-not-leak",
            "fixture-credential",
            "fixture-secret",
            "org-a",
            "http://",
            "human",
        ] {
            assert!(
                !event.to_string().contains(private),
                "private diagnostic payload"
            );
        }
    }
    (result, events)
}
