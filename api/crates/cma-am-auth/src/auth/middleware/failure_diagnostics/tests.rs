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
            .event_format(crate::diagnostic_format::json())
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
                StoreError::ReferenceUnavailable("must-not-leak-reference".into()),
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
        assert_eq!(events.len(), 4);
        for (event, class) in events.iter().zip([
            "unavailable",
            "malformed",
            "revision_conflict",
            "reference_unavailable",
        ]) {
            assert_eq!(event["fields"]["error_class"], class);
            assert_eq!(event["fields"]["request_hash"], "a".repeat(64));
            assert_eq!(event["fields"]["auth_stage"], "request_resolution");
            assert_eq!(event["fields"]["dependency"], "storage");
            assert_eq!(event["fields"]["reason"], "unavailable");
            assert!(event.get("span").is_none());
            assert!(event.get("spans").is_none());
            assert!(!event.to_string().contains("123456"));
            assert!(!event.to_string().contains("654321"));
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Filter {
    PerLayer,
    Env,
}

fn dispatch(log: Log, json: bool, exclude_root: bool, filter: Filter) -> tracing::Dispatch {
    match filter {
        Filter::PerLayer => {
            let filter = tracing_subscriber::filter::filter_fn(move |metadata| {
                !exclude_root || metadata.name() != "authentication_failure"
            });
            let layer = tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(move || log.clone());
            if json {
                tracing::Dispatch::new(
                    tracing_subscriber::registry().with(
                        layer
                            .json()
                            .event_format(crate::diagnostic_format::json())
                            .with_filter(filter),
                    ),
                )
            } else {
                tracing::Dispatch::new(
                    tracing_subscriber::registry().with(layer.with_filter(filter)),
                )
            }
        }
        Filter::Env => {
            let directive = if exclude_root {
                "info,[authentication_failure]=off"
            } else {
                "info"
            };
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_env_filter(directive)
                .with_writer(move || log.clone());
            if json {
                tracing::Dispatch::new(
                    subscriber
                        .json()
                        .event_format(crate::diagnostic_format::json())
                        .finish(),
                )
            } else {
                tracing::Dispatch::new(subscriber.finish())
            }
        }
    }
}

fn emit_private_classes() {
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
        StoreError::ReferenceUnavailable("must-not-leak-reference".into()),
    ] {
        context.sync_scope(|| {
            record(
                &error,
                Failure::new(Dependency::Storage, Reason::Unavailable),
            )
        });
    }
}

fn assert_private_output(text: &str, json: bool, suppressed: bool) {
    assert_private_output_with_root(text, json, suppressed, true);
}

fn assert_private_output_with_root(text: &str, json: bool, suppressed: bool, root_visible: bool) {
    for private in ["must-not-leak", "private_parent", "123456", "654321"] {
        assert!(!text.contains(private), "private field {private}: {text}");
    }
    if suppressed {
        assert!(text.is_empty(), "filtered root emitted: {text}");
        return;
    }
    assert_eq!(text.lines().count(), 4, "{text}");
    for (line, class) in text.lines().zip([
        "unavailable",
        "malformed",
        "revision_conflict",
        "reference_unavailable",
    ]) {
        if json {
            let event: Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                event["fields"],
                json!({
                    "message": "Request authentication failed",
                    "event": "authentication_failure",
                    "request_hash": "a".repeat(64),
                    "auth_stage": "request_resolution",
                    "error_class": class,
                    "dependency": "storage",
                    "reason": "unavailable",
                })
            );
            assert!(event.get("span").is_none());
            assert!(event.get("spans").is_none());
        } else {
            assert_eq!(
                line.contains("authentication_failure:"),
                root_visible,
                "{line}"
            );
            for field in [
                "Request authentication failed",
                "event=\"authentication_failure\"",
                "auth_stage=\"request_resolution\"",
                "dependency=\"storage\"",
                "reason=\"unavailable\"",
                &format!("request_hash=\"{}\"", "a".repeat(64)),
                &format!("error_class=\"{class}\""),
            ] {
                assert!(line.contains(field), "missing {field}: {line}");
            }
        }
    }
}

mod multilayer;

#[test]
fn current_dispatch_controls_privacy_with_coexisting_subscribers() {
    for json in [true, false] {
        for filter in [Filter::PerLayer, Filter::Env] {
            for (exclude_root, coexisting) in [(false, false), (true, false), (true, true)] {
                // Registration, not concurrent execution, triggers shared callsite
                // interest. Warm the real helper and retain this Dispatch until
                // after the selected subscriber has finished; no scheduler race.
                let other_log = Log::default();
                let other = coexisting.then(|| {
                    let other = dispatch(other_log.clone(), json, false, filter);
                    tracing::dispatcher::with_default(&other, emit_private_classes);
                    other
                });
                let log = Log::default();
                let selected = dispatch(log.clone(), json, exclude_root, filter);
                tracing::dispatcher::with_default(&selected, emit_private_classes);
                drop(selected);
                drop(other);
                let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
                // This EnvFilter directive does not suppress the safe root in
                // this fixture. Preserve its observed emission/ancestry control.
                let suppressed = exclude_root && matches!(filter, Filter::PerLayer);
                assert_private_output(&text, json, suppressed);
                let warmup = String::from_utf8(other_log.0.lock().unwrap().clone()).unwrap();
                if coexisting {
                    assert_private_output(&warmup, json, false);
                } else {
                    assert!(warmup.is_empty());
                }
            }
        }
    }
}
