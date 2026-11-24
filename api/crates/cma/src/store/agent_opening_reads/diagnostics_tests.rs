use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tracing::{Level, Metadata, Subscriber};
use tracing_subscriber::{
    filter::dynamic_filter_fn, layer::SubscriberExt, registry::LookupSpan, EnvFilter, Layer,
};

const TARGET: &str = "cma::store::agent_opening_reads";
const CANARY: &str = "synthetic-opening-span-canary";

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

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
    tracing::dispatcher::with_default(&subscriber, emit_under_private_parent);
    log.text()
}

fn emit_under_private_parent() {
    let private = tracing::warn_span!(target: "opening_private_fixture", "opening_private_parent", payload = CANARY);
    assert!(!private.is_disabled());
    let _entered = private.enter();
    super::decision("usage_summary");
}

fn allows_events_only(meta: &Metadata<'_>) -> bool {
    *meta.level() == Level::WARN || (meta.target() == TARGET && meta.is_event())
}

fn assert_private_output(text: &str) {
    assert_eq!(text.lines().count(), 1);
    assert!(!text.contains(CANARY));
    assert!(!text.contains("opening_private_parent"));
}

fn assert_json_fields(event: &Value) {
    assert_eq!(event["target"], TARGET);
    assert_eq!(event["level"], "DEBUG");
    assert_eq!(
        event["fields"],
        json!({
            "message":"Agent opening legacy projection decision",
            "outcome":"usage_summary"
        })
    );
}

fn assert_text_fields(text: &str) {
    assert!(text.contains("DEBUG "));
    assert!(text.contains(&format!("{TARGET}:")));
    assert!(text.contains("outcome=\"usage_summary\""));
    assert!(text.contains("Agent opening legacy projection decision"));
}

#[test]
fn opening_decision_omits_ambient_private_span_from_json_and_text() {
    for json_output in [true, false] {
        let text = capture(
            json_output,
            EnvFilter::try_new(format!("warn,{TARGET}=debug")).unwrap(),
        );
        assert_private_output(&text);
        if json_output {
            let event: Value = serde_json::from_str(&text).unwrap();
            let root = json!({"name":"agent_opening_decision"});
            assert_json_fields(&event);
            assert_eq!(event["span"], root);
            assert_eq!(event["spans"], json!([root]));
        } else {
            assert_text_fields(&text);
        }
    }
}

#[test]
fn opening_decision_honors_disabled_debug_and_event_only_filters() {
    for json_output in [true, false] {
        assert!(capture(json_output, EnvFilter::try_new("warn").unwrap()).is_empty());
        let events_only = Log::default();
        let subscriber = tracing_subscriber::registry().with(diagnostic_layer(
            events_only.clone(),
            json_output,
            true,
        ));
        tracing::subscriber::with_default(subscriber, emit_under_private_parent);
        assert!(events_only.text().is_empty());
    }
}

fn diagnostic_layer<S>(log: Log, json: bool, events_only: bool) -> Box<dyn Layer<S> + Send + Sync>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup> + 'static,
{
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(move || log.clone());
    let layer = if json {
        layer
            .json()
            .event_format(crate::diagnostic_format::json())
            .boxed()
    } else {
        layer.event_format(crate::diagnostic_format::text()).boxed()
    };
    layer
        .with_filter(dynamic_filter_fn(move |meta, _| {
            allows_events_only(meta) || (!events_only && meta.target() == TARGET)
        }))
        .boxed()
}

#[test]
fn opening_decision_requires_a_visible_root_in_each_writer() {
    for json_output in [true, false] {
        let accepting = Log::default();
        let events_only = Log::default();
        let subscriber = tracing_subscriber::registry()
            .with(diagnostic_layer(accepting.clone(), json_output, false))
            .with(diagnostic_layer(events_only.clone(), json_output, true));
        tracing::subscriber::with_default(subscriber, emit_under_private_parent);

        assert!(events_only.text().is_empty());
        let text = accepting.text();
        assert_private_output(&text);
        if json_output {
            let event: Value = serde_json::from_str(&text).unwrap();
            assert_json_fields(&event);
            assert!(event.get("span").is_none());
            assert!(event.get("spans").is_none());
        } else {
            assert_text_fields(&text);
        }
    }
}
