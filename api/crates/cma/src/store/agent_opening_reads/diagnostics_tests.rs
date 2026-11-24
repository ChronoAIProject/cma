use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tracing_subscriber::{layer::SubscriberExt, EnvFilter, Layer};

const TARGET: &str = "cma::store::agent_opening_reads";
const CANARY: &str = "synthetic-opening-span-canary";

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

fn capture(
    json: bool,
    filter: impl tracing_subscriber::layer::Filter<tracing_subscriber::Registry> + Send + Sync + 'static,
) -> String {
    let log = Log::default();
    let writer = log.clone();
    let layout = tracing_subscriber::fmt::layer()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone());
    let subscriber = if json {
        tracing::Dispatch::new(
            tracing_subscriber::registry().with(layout.json().with_filter(filter)),
        )
    } else {
        tracing::Dispatch::new(tracing_subscriber::registry().with(layout.with_filter(filter)))
    };
    tracing::dispatcher::with_default(&subscriber, || {
        let private = tracing::warn_span!(target: "opening_private_fixture", "opening_private_parent", payload = CANARY);
        assert!(!private.is_disabled());
        let _entered = private.enter();
        super::decision("usage_summary");
    });
    let bytes = log.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn opening_decision_omits_ambient_private_span_from_json_and_text() {
    for json_output in [true, false] {
        let text = capture(
            json_output,
            EnvFilter::try_new(format!("warn,{TARGET}=debug")).unwrap(),
        );
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains(CANARY));
        assert!(!text.contains("opening_private_parent"));
        if json_output {
            let event: Value = serde_json::from_str(&text).unwrap();
            let root = json!({"name":"agent_opening_decision"});
            assert_eq!(event["target"], TARGET);
            assert_eq!(event["level"], "DEBUG");
            assert_eq!(
                event["fields"],
                json!({
                    "message":"Agent opening legacy projection decision",
                    "outcome":"usage_summary"
                })
            );
            assert_eq!(event["span"], root);
            assert_eq!(event["spans"], json!([root]));
        } else {
            assert!(text.contains(&format!("DEBUG {TARGET}:")));
            assert!(text.contains("outcome=\"usage_summary\""));
            assert!(text.contains("Agent opening legacy projection decision"));
        }
    }
}

#[test]
fn opening_decision_honors_disabled_debug_and_event_only_filters() {
    for json_output in [true, false] {
        assert!(capture(json_output, EnvFilter::try_new("warn").unwrap()).is_empty());
        let events_only = tracing_subscriber::filter::filter_fn(|meta| {
            *meta.level() == tracing::Level::WARN || (meta.target() == TARGET && meta.is_event())
        });
        assert!(capture(json_output, events_only).is_empty());
    }
}
