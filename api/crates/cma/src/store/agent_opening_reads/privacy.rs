use super::decision;
use std::sync::{Arc, Mutex};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter};

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

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture(json: bool, filter: &str, disable_root: bool) -> String {
    let output = Output::default();
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        // Keep the private parent entered while permitting events alone. A
        // disabled safety span must suppress the event, not expose that parent.
        .with(tracing_subscriber::filter::filter_fn(move |metadata| {
            !disable_root || metadata.is_event() || metadata.name() == "private_parent"
        }));
    let layer = tracing_subscriber::fmt::layer()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone());
    let dispatch = if json {
        tracing::Dispatch::new(subscriber.with(layer.json()))
    } else {
        tracing::Dispatch::new(subscriber.with(layer))
    };
    tracing::dispatcher::with_default(&dispatch, || {
        let private = tracing::warn_span!(
            target: "private_context",
            "private_parent",
            private_canary = "synthetic-private-opening"
        );
        let _entered = private.enter();
        decision("cohort_excluded");
    });
    let bytes = output.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

fn assert_safe(log: &str, json: bool) {
    assert_eq!(log.lines().count(), 1, "{log}");
    for private in [
        "private_canary",
        "synthetic-private-opening",
        "private_parent",
    ] {
        assert!(!log.contains(private), "{log}");
    }
    assert!(log.contains("cohort_excluded"), "{log}");
    if json {
        let event: serde_json::Value = serde_json::from_str(log).unwrap();
        assert_eq!(event["level"], "DEBUG");
        assert_eq!(event["target"], "cma::store::agent_opening_reads");
        assert_eq!(
            event["fields"],
            serde_json::json!({
                "message": "Agent opening legacy projection decision",
                "outcome": "cohort_excluded"
            })
        );
        assert_eq!(
            event["span"],
            serde_json::json!({"name": "agent_opening_decision"})
        );
        assert_eq!(
            event["spans"],
            serde_json::json!([{"name": "agent_opening_decision"}])
        );
    }
}

#[test]
fn decision_json_does_not_inherit_private_parent() {
    assert_safe(&capture(true, "debug", false), true);
}

#[test]
fn decision_privacy_preserves_target_filters_and_suppresses_disabled_roots() {
    for json in [false, true] {
        for filter in ["debug", "warn,cma::store::agent_opening_reads=debug"] {
            assert_safe(&capture(json, filter, false), json);
            assert!(capture(json, filter, true).is_empty());
        }
        for filter in ["info", "debug,cma::store::agent_opening_reads=off"] {
            assert!(capture(json, filter, false).is_empty());
        }
    }
}
