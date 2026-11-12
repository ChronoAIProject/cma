use super::*;
use crate::store::mongo_diagnostics::{output, tests::Output, DiagnosticContext};
use mongodb::options::ServerAddress;

fn attributes() -> Vec<KeyValue> {
    vec![
        KeyValue::new("db.command.name", "find"),
        KeyValue::new("db.collection.name", "agents"),
        KeyValue::new("db.mongodb.driver_connection_id", 7_i64),
        KeyValue::new("server.address", "localhost"),
        KeyValue::new("server.port", 27017_i64),
        KeyValue::new("network.transport", "tcp"),
        KeyValue::new("db.mongodb.lsid", "private-session-value"),
        // These initial attributes must not prove a transaction or success:
        // retryable writes carry txn_number; getMore starts with a cursor ID.
        KeyValue::new("db.mongodb.txn_number", 12345_i64),
        KeyValue::new("db.mongodb.cursor_id", 12345_i64),
        KeyValue::new("db.query.text", "private-payload".repeat(10_000)),
        KeyValue::new("db.namespace", "private-database"),
    ]
}
fn start(provider: &Provider) -> MetadataSpan {
    provider.build_with_context(
        SpanBuilder::from_name("private-operation-name").with_attributes(attributes()),
        &Context::new(),
    )
}
#[test]
fn connection_metadata_matches_cmap_without_retaining_address() {
    let expected = crate::store::mongo_latency::connection_key(
        &ServerAddress::Tcp {
            host: "localhost".into(),
            port: None,
        },
        7,
    );
    assert_eq!(connection(&attributes()), Some(expected));
    assert!(std::mem::size_of::<MetadataSpan>() < 256);
    let mut malformed = attributes();
    malformed.retain(|a| a.key.as_str() != "server.port");
    assert_eq!(connection(&malformed), None);
}
#[tokio::test]
async fn metadata_is_private_and_completion_is_once_with_unknowns_explicit() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let diagnostics = Diagnostics::default();
    let provider = Provider(diagnostics.clone());
    let mut span = DiagnosticContext::background("scheduler")
        .with_phase("probe")
        .scope(async { start(&provider) })
        .await;
    span.started -= super::super::SLOW;
    span.set_attribute(KeyValue::new("exception.message", "private-error"));
    span.set_attribute(KeyValue::new("db.mongodb.cursor_id", 12345_i64));
    span.end();
    drop(span);
    let mut failed = start(&provider);
    failed.set_status(Status::error("private-failure"));
    drop(failed);
    let mut cancelled = start(&provider);
    cancelled.started -= super::super::SLOW;
    drop(cancelled);
    output::flush();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains("private"));
    assert!(!logs.contains("12345"));
    assert_eq!(logs.matches("event=\"succeeded\"").count(), 1);
    assert_eq!(logs.matches("event=\"failed\"").count(), 1);
    assert_eq!(logs.matches("event=\"completed\"").count(), 1);
    assert!(logs.contains("worker=\"scheduler\""));
    assert!(logs.contains("phase=\"probe\""));
    assert!(logs.contains("docs_returned_known=false"));
    assert!(logs.contains("transaction_known=false"));
    assert!(!logs.contains("transaction=true"));
    assert!(logs.contains("outcome_known=false"));
    assert!(diagnostics.0.lock().unwrap().commands.is_empty());
}
#[tokio::test]
async fn concurrent_contexts_remain_distinct_and_operation_spans_are_ignored() {
    let diagnostics = Diagnostics::default();
    let provider = Provider(diagnostics.clone());
    let a = DiagnosticContext::background("scheduler").scope(async {
        tokio::task::yield_now().await;
        start(&provider)
    });
    let b = DiagnosticContext::background("reconciliation").scope(async {
        tokio::task::yield_now().await;
        start(&provider)
    });
    let (a, b) = tokio::join!(a, b);
    {
        let state = diagnostics.0.lock().unwrap();
        assert_eq!(state.commands.len(), 2);
        assert!(state
            .commands
            .values()
            .any(|c| c.context.worker == "scheduler"));
        assert!(state
            .commands
            .values()
            .any(|c| c.context.worker == "reconciliation"));
    }
    let operation = provider.build_with_context(
        SpanBuilder::from_name("find private-database.agents")
            .with_attributes([KeyValue::new("db.operation.name", "find")]),
        &Context::new(),
    );
    assert!(!operation.is_recording());
    drop((a, b, operation));
    assert!(diagnostics.0.lock().unwrap().commands.is_empty());
}

#[tokio::test]
async fn readiness_spans_keep_attempt_joins_and_never_promote_unknown_outcomes() {
    use crate::readiness_diagnostics::{fixture::Capture, record::Record, Attempt};
    use std::time::Duration;

    for (event, phase) in [
        ("command_succeeded", "command_completed"),
        ("command_failed", "command_failed"),
        ("command_completed", "command"),
    ] {
        let capture = Capture::new();
        let diagnostics = Diagnostics {
            2: Some(capture.recorder.clone()),
            ..Default::default()
        };
        let provider = Provider(diagnostics.clone());
        let mut attempt = Attempt::start(
            Some(capture.recorder.clone()),
            tokio::time::Instant::now(),
            Duration::from_secs(2),
        );
        let check = attempt.check();
        let mut span = attempt
            .run(DiagnosticContext::background("health_monitor").scope(async { start(&provider) }))
            .await;
        match event {
            "command_succeeded" => span.set_attribute(KeyValue::new("db.mongodb.cursor_id", 7_i64)),
            "command_failed" => span.set_status(Status::error("private-driver-error")),
            _ => (),
        }
        // Completion can run after the originating task-local scope is gone.
        span.end();
        drop(span);
        attempt.finish("deadline");
        let snapshots = capture.snapshots();
        let snapshot = &snapshots[0];
        assert!(snapshot.active.is_empty());
        assert_eq!(snapshot.rows.len(), 2);
        let started = &snapshot.rows[0];
        let completed = &snapshot.rows[1];
        assert_eq!(started.event, "command_started");
        assert_eq!(completed.event, event);
        assert_ne!(started.metadata.operation, 0);
        assert_eq!(started.metadata.operation, completed.metadata.operation);
        for row in &snapshot.rows {
            assert_eq!(row.metadata.check, check);
            assert_eq!(row.metadata.context.worker, "health_monitor");
            assert!(!row.metadata.transaction_known);
        }
        assert_eq!(
            completed.metadata.outcome_known,
            event != "command_completed"
        );
        assert!(capture.records().iter().any(|r| matches!(
            r,
            Record::Attempt { outcome: "deadline", phase: actual, .. } if *actual == phase
        )));
        assert!(diagnostics.0.lock().unwrap().commands.is_empty());
    }
}
