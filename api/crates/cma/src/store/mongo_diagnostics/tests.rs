use super::*;
use mongodb::bson::doc;
use std::io::Write;
#[derive(Clone, Default)]
pub(super) struct Output(pub(super) Arc<Mutex<Vec<u8>>>);
impl Write for Output {
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
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[tokio::test]
async fn slow_success_failure_and_hold_emit_safe_fields_without_parent_values() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        // JSON 0.3.23 otherwise falls back to the current span for root events.
        .with_current_span(false)
        .with_span_list(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!("private", identity = "never-log-this-owner");
    let _entered = span.enter();
    let diagnostics = Diagnostics::default();
    let context = DiagnosticContext {
        route: "GET /api/v1/sandboxes/{id}".into(),
        request_hash: "a".repeat(64).into(),
        worker: "request",
        phase: "hierarchy",
    };
    let body = doc! { "find": "sandboxes", "filter": {"owner": "never-log-this-owner"}, "lsid": {"id": "never-log-session"}, "autocommit": false };
    let command = context
        .clone()
        .scope(async { metadata("find", &body, false) })
        .await;
    assert!(command.session && command.transaction);
    assert_eq!(command.collection, "sandboxes");
    assert_eq!(
        metadata("private-command", &doc! {}, false).command,
        "other"
    );
    assert_eq!(
        metadata("find", &doc! {"find":"private-collection"}, false).collection,
        "other"
    );
    assert_eq!(
        docs_returned(&doc! {"cursor":{"firstBatch":[{"private":"value"},{}]}}),
        Some(2)
    );
    assert_eq!(docs_returned(&doc! {"cursor":{"nextBatch":[]}}), Some(0));
    assert_eq!(docs_returned(&doc! {"private":1}), None);
    for id in 1..=2 {
        diagnostics
            .0
            .lock()
            .unwrap()
            .commands
            .insert((1, id), command.clone());
        diagnostics.finished(1, id, Duration::from_millis(101), Some(2), id == 2);
    }
    // Check-in runs outside the originating task; it must retain its labels.
    diagnostics.0.lock().unwrap().holds.insert(
        1,
        Hold {
            hash: "c".repeat(64).into(),
            start: Instant::now() - SLOW,
            context,
            last: Some(command),
            commands: 1,
            sampled: false,
        },
    );
    diagnostics.returned(1, false);
    let bytes = output.0.lock().unwrap().clone();
    let logs = String::from_utf8(bytes).unwrap();
    let events: Vec<serde_json::Value> = logs
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 5);
    for event in &events {
        let fields = &event["fields"];
        for key in [
            "route",
            "request_hash",
            "hold_hash",
            "worker",
            "phase",
            "command",
            "collection",
            "duration_us",
            "session",
            "transaction",
            "connection_hold_elapsed_us",
            "sample_every",
            "deferred",
        ] {
            assert!(!fields[key].is_null(), "{key}: {fields}");
        }
        assert_eq!(fields["phase"], "hierarchy");
        assert!(event.get("span").is_none());
        assert!(event.get("spans").is_none());
    }
    for value in [
        "never-log",
        "private-command",
        "private-collection",
        "filter",
        "lsid",
    ] {
        assert!(!logs.contains(value), "{logs}");
    }
    assert_eq!(events[1]["fields"]["event"], "succeeded");
    assert_eq!(events[1]["fields"]["docs_returned"], 2);
    assert_eq!(events[3]["fields"]["event"], "failed");
    assert_eq!(events[4]["fields"]["event"], "checked_in");
}
#[test]
fn production_text_format_excludes_caller_fields_from_events_and_summaries() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!("private_parent", owner = "private-owner-sentinel");
    let _entered = span.enter();
    let command = metadata(
        "find",
        &doc! {"find":"agents", "filter":{"owner":"private-filter"}},
        false,
    );
    command.emit("failed", SLOW, None, 0, false);
    let mut totals = Summary::default();
    totals.record(&command, "command", SLOW, None, true);
    summary::emit(Some(totals.drain()));
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 2);
    assert!(logs.contains("MongoDB diagnostic event"));
    assert!(logs.contains("MongoDB attributed totals"));
    assert!(!logs.contains("private"), "{logs}");
}
#[test]
fn fast_events_are_sampled_but_totals_and_closed_connection_cleanup_are_complete() {
    let diagnostics = Diagnostics::default();
    for id in 0..10_000 {
        diagnostics
            .0
            .lock()
            .unwrap()
            .commands
            .insert((1, id), metadata("find", &doc! {"find":"agents"}, false));
        diagnostics.finished(1, id, Duration::from_micros(10), Some(1), false);
    }
    assert!(diagnostics.0.lock().unwrap().commands.is_empty());
    diagnostics
        .0
        .lock()
        .unwrap()
        .commands
        .insert((2, 1), metadata("find", &doc! {}, false));
    diagnostics.0.lock().unwrap().holds.insert(
        2,
        Hold {
            hash: "c".repeat(64).into(),
            start: Instant::now(),
            context: DiagnosticContext::default(),
            last: None,
            commands: 0,
            sampled: false,
        },
    );
    diagnostics.returned(2, true);
    assert!(diagnostics.0.lock().unwrap().commands.is_empty());
    assert!(diagnostics.0.lock().unwrap().holds.is_empty());
}
#[test]
fn callback_overhead_is_bounded_without_log_output() {
    let diagnostics = Diagnostics::default();
    let command = metadata("find", &doc! {"find":"agents"}, false);
    let start = Instant::now();
    for id in 0..100_000 {
        diagnostics
            .0
            .lock()
            .unwrap()
            .commands
            .insert((1, id), command.clone());
        diagnostics.finished(1, id, Duration::ZERO, Some(1), false);
    }
    let per_command_ns = start.elapsed().as_nanos() / 100_000;
    eprintln!("diagnostic_callback_ns={per_command_ns}");
    // Debug-build ceiling; the real-Mongo gate separately measures eager BSON
    // decoding and I/O with the complete monitor installed.
    assert!(per_command_ns < 100_000, "callback ns={per_command_ns}");
}
