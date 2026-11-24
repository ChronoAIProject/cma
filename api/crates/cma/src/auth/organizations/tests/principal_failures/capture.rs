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

impl Log {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

pub(super) async fn capture<F: Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let plain = Log::default();
    let plain_writer = plain.clone();
    let hidden_root = Log::default();
    let hidden_writer = hidden_root.clone();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .event_format(crate::diagnostic_format::json())
                .with_writer(move || writer.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(crate::diagnostic_format::text())
                .with_ansi(false)
                .with_writer(move || plain_writer.clone()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .event_format(crate::diagnostic_format::json())
                .with_writer(move || hidden_writer.clone())
                .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                    meta.name() != "organization_principal"
                })),
        );
    let result = future.with_subscriber(subscriber).await;
    let json = log.text();
    let text = plain.text();
    let event_name = "organization_principal_failure";
    assert!(!hidden_root.text().contains(event_name));
    for output in [&json, &text] {
        for line in output.lines().filter(|line| line.contains(event_name)) {
            for private in [
                "must-not-leak",
                "fixture-credential",
                "fixture-secret",
                "http://",
                "human",
            ] {
                assert!(!line.contains(private), "{line}");
            }
        }
    }
    let events: Vec<Value> = json
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == event_name)
        .collect();
    assert_eq!(
        text.lines()
            .filter(|line| line.contains(event_name))
            .count(),
        events.len()
    );
    for event in &events {
        assert!(event.get("span").is_none());
        assert!(event.get("spans").is_none());
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
            .event_format(crate::diagnostic_format::json())
            .with_writer(move || writer.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| {
                meta.name() != "organization_principal"
            })),
    );
    let result = future.with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (result, text)
}
