//! Parent spans must not undo the routing log's event-identifier hashing.
use super::custom::{build_app, event};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, Layer};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn authenticated_custom_ingress_hashes_ids_in_parent_and_child_spans() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_span_events(FmtSpan::CLOSE)
            .with_writer(move || writer.clone())
            .with_filter(cmaeg::log_layer_filter(Some("trace"))),
    );
    tracing::subscriber::with_default(subscriber, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                for (enabled, legacy, targets, status) in [
                    (true, false, "reviewer,main", 202),
                    (true, false, "producer-private-token", 400),
                    (false, false, "reviewer", 503),
                    (true, true, "reviewer", 202),
                ] {
                    let app = build_app(enabled, |raw| {
                        if legacy {
                            raw.as_object_mut().unwrap().remove("routing");
                            raw["secrets"]
                                .as_object_mut()
                                .unwrap()
                                .remove("consumer_keys");
                        }
                    })
                    .await;
                    let mut event = event(Some(targets));
                    event["id"] = "private-person@example.test".into();
                    event["data"] = serde_json::json!({"text":"message-private-sentinel"});
                    let response = app.custom(event, false).await;
                    assert_eq!(response.status(), status);
                    let _: serde_json::Value = response.json().await.unwrap();
                    app.idle().await;
                }
            });
    });
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for secret in [
        "private-person@example.test",
        "producer-private-token",
        "message-private-sentinel",
        "Zml4dHVyZQ==",
        "cmaeg1.ev.",
        "nyx_fixture_",
        "fixture-channel-key",
    ] {
        assert!(!logs.contains(secret), "leaked {secret}: {logs}");
    }
    let hash = cmaeg::telemetry::channel_hash("private-person@example.test");
    for span in ["cmaeg.verify", "cmaeg.route", "cmaeg.fanout", "cmaeg.turn"] {
        assert!(
            logs.lines()
                .any(|line| line.contains(span) && line.contains(&hash)),
            "missing {span}: {logs}"
        );
    }
    assert_eq!(logs.matches("routing decision").count(), 4, "{logs}");
    assert!(logs.contains("resolution=\"legacy\""), "{logs}");
    assert!(logs.contains("kill-switch routing refusal"), "{logs}");
}
