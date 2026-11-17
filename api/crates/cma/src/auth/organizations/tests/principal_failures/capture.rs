use super::*;
use std::future::Future;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[derive(Clone, Default)]
struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) async fn capture<F: Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let plain = Log::default();
    let plain_writer = plain.clone();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(move || writer.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .without_time()
                .with_writer(move || plain_writer.clone()),
        );
    let result = future.with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "organization_principal_failure")
        .collect();
    for event in &events {
        assert_eq!(event["span"], json!({"name":"organization_principal"}));
        assert_eq!(event["spans"], json!([{"name":"organization_principal"}]));
        for private in [
            "must-not-leak",
            "fixture-credential",
            "fixture-secret",
            "http://",
            "human",
        ] {
            assert!(!event.to_string().contains(private), "{event}");
        }
    }
    // A local run can retain actual emitted text for the two external collector
    // owners to check, without a production caller or a copied parser in CMA.
    if let Some(path) = std::env::var_os("CMA_TEST_PRINCIPAL_EVENTS") {
        use std::io::Write;
        let plain = String::from_utf8(plain.0.lock().unwrap().clone()).unwrap();
        let mut output = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in plain
            .lines()
            .filter(|line| line.contains("event=\"organization_principal_failure\""))
        {
            for private in [
                "must-not-leak",
                "fixture-credential",
                "fixture-secret",
                "http://",
                "human",
            ] {
                assert!(!line.contains(private));
            }
            output.write_all(format!("{line}\n").as_bytes()).unwrap();
        }
    }
    (result, events)
}

pub(super) fn context(hash: &str) -> DiagnosticContext {
    DiagnosticContext {
        request_hash: hash.to_owned().into(),
        ..DiagnosticContext::default()
    }
}

pub(super) fn fields(events: &[Value], phase: &str, reason: &str) -> Value {
    assert_eq!(events.len(), 1, "{events:?}");
    let fields = events[0]["fields"].clone();
    assert_eq!(fields["phase"], phase);
    assert_eq!(fields["reason"], reason);
    fields
}

pub(super) async fn disabled_root<F: Future>(future: F) -> (F::Output, String) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(move || writer.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                meta.name() != "organization_principal"
            })),
    );
    let result = future.with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (result, text)
}
