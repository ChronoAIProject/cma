use super::*;
use layer2_core::dependency_diagnostics::{Dependency, Reason};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tracing_subscriber::{layer::SubscriberExt, Layer};

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

#[test]
fn typed_classes_are_private_even_with_selective_span_filters() {
    for root_enabled in [true, false] {
        let log = Log::default();
        let writer = log.clone();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(move || writer.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
                root_enabled || metadata.name() != "authentication_failure"
            }));
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let private = tracing::info_span!("private_parent", secret = "must-not-leak-span");
            let _entered = private.enter();
            let context = DiagnosticContext {
                request_hash: "a".repeat(64).into(),
                route: "must-not-leak-route".into(),
                ..DiagnosticContext::default()
            };
            for error in [
                StoreError::Unavailable("must-not-leak-error".into()),
                StoreError::Malformed("must-not-leak-body".into()),
                StoreError::RevisionConflict {
                    expected: 123456,
                    actual: 654321,
                },
            ] {
                context.sync_scope(|| {
                    record(
                        &error,
                        Failure::new(Dependency::Storage, Reason::Unavailable),
                    )
                });
            }
        });
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(!text.contains("must-not-leak"));
        if !root_enabled {
            assert!(text.is_empty(), "{text}");
            continue;
        }
        let events: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        for (event, class) in events
            .iter()
            .zip(["unavailable", "malformed", "revision_conflict"])
        {
            assert_eq!(event["fields"]["error_class"], class);
            assert_eq!(event["fields"]["request_hash"], "a".repeat(64));
            assert_eq!(event["fields"]["auth_stage"], "request_resolution");
            assert_eq!(event["fields"]["dependency"], "storage");
            assert_eq!(event["fields"]["reason"], "unavailable");
            assert_eq!(event["span"], json!({"name":"authentication_failure"}));
            assert_eq!(event["spans"], json!([{"name":"authentication_failure"}]));
            assert!(!event.to_string().contains("123456"));
            assert!(!event.to_string().contains("654321"));
        }
    }
}
