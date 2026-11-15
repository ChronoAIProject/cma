use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{layer::Context, prelude::*, Layer};

use super::StartupTiming;

#[derive(Clone, Debug, Default)]
pub(in crate::hosted) struct Record {
    pub fields: BTreeMap<String, String>,
    pub contextual: bool,
    pub has_parent: bool,
}

impl Visit for Record {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().into(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().into(), value.into());
    }
}

#[derive(Clone, Default)]
pub(in crate::hosted) struct Capture(Arc<Mutex<Vec<Record>>>);

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut record = Record {
            contextual: event.is_contextual(),
            has_parent: event.parent().is_some(),
            ..Record::default()
        };
        event.record(&mut record);
        if record.fields.contains_key("attempt_id") {
            self.0.lock().unwrap().push(record);
        }
    }
}

impl Capture {
    pub fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        tracing_subscriber::registry().with(self.clone())
    }

    pub fn records(&self) -> Vec<Record> {
        self.0.lock().unwrap().clone()
    }

    pub fn subphases(&self) -> Vec<Record> {
        self.records()
            .into_iter()
            .filter(|row| {
                row.fields.get("phase").map(String::as_str) == Some("protocol_session_subphase")
            })
            .collect()
    }
}

#[tokio::test]
async fn nested_success_and_error_preserve_the_enclosing_clock_and_results() {
    let capture = Capture::default();
    async {
        let mut timing = StartupTiming::for_agent("bounded-digest".into());
        timing.last = Instant::now() - Duration::from_secs(3);
        assert_eq!(
            timing
                .subphase("fresh_new", "session_new", async { Ok::<_, &str>(17) })
                .await,
            Ok(17)
        );
        assert_eq!(
            timing
                .subphase("fresh_new", "activate_session", async {
                    Err::<(), _>("private-error")
                })
                .await,
            Err("private-error")
        );
        timing.completed("protocol_session");
    }
    .with_subscriber(capture.subscriber())
    .await;
    let records = capture.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].fields["outcome"], "ok");
    assert_eq!(records[1].fields["outcome"], "error");
    assert!(records[2].fields["elapsed_ms"].parse::<u64>().unwrap() >= 3_000);
    assert!(records
        .iter()
        .all(|r| r.fields["attempt_id"] == records[0].fields["attempt_id"]));
    assert!(!format!("{records:?}").contains("private-error"));
}

#[tokio::test]
async fn cancellation_emits_once_without_inheriting_the_callers_span() {
    let capture = Capture::default();
    async {
        let span = tracing::info_span!("private-parent", secret = "private-parent-value");
        let _entered = span.enter();
        let timing = StartupTiming::for_agent("bounded-digest".into());
        let mut waiting = Box::pin(timing.subphase(
            "fresh_load",
            "session_load",
            std::future::pending::<Result<(), ()>>(),
        ));
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
        drop(waiting);
    }
    .with_subscriber(capture.subscriber())
    .await;
    let records = capture.subphases();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].fields["outcome"], "cancelled");
    assert!(!records[0].contextual);
    assert!(!records[0].has_parent);
    assert!(!format!("{records:?}").contains("private-parent"));
}
