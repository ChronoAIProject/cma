use super::*;
use crate::telemetry::turn_trace::{Context, Operation, Outcome, SpanId, Stage};
use serde_json::Value;
use std::sync::{Condvar, Mutex};

struct Capture(mpsc::Sender<Vec<u8>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture() -> (Output, Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    (start(Capture(tx)).unwrap(), rx)
}

fn context() -> Context {
    Context {
        turn_id: Some(Uuid::new_v4()),
        span_id: SpanId::fresh(),
        parent_span_id: None,
        request_id: Uuid::new_v4(),
        ingress_request_id: Some(Uuid::new_v4()),
    }
}

fn read(receiver: &Receiver<Vec<u8>>) -> Value {
    let bytes = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("trace line");
    assert!(bytes.len() <= super::super::record::MAX_RECORD_BYTES);
    assert_eq!(bytes.last(), Some(&b'\n'));
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn gateway_platform_and_egress_share_captured_paired_context() {
    let (output, lines) = capture();
    let source = context();
    let operation = Operation::start(output.writer.clone(), Stage::Gateway, Some(source), None);
    let gateway = operation.context();
    let child = Operation::start(
        output.writer.clone(),
        Stage::Platform,
        Some(gateway),
        Some(2),
    );
    let mut headers = axum::http::HeaderMap::new();
    child.headers(&mut headers);
    let child_context = child.context();
    assert_eq!(
        headers["traceparent"],
        format!(
            "00-{}-{}-01",
            source.turn_id.unwrap().simple(),
            child_context.span_id
        )
    );
    assert_eq!(
        headers["x-request-id"],
        child_context.request_id.to_string()
    );
    child.finish(Outcome::Unknown, Some(502));
    operation.finish(Outcome::Completed, Some(200));
    let records = [read(&lines), read(&lines), read(&lines), read(&lines)];
    assert_eq!(records[0]["phase"], "start");
    assert_eq!(records[0]["span_id"], records[3]["span_id"]);
    assert_eq!(records[0]["request_id"], records[3]["request_id"]);
    assert_eq!(records[1]["span_id"], records[2]["span_id"]);
    assert_eq!(records[1]["request_id"], records[2]["request_id"]);
    assert_eq!(records[1]["parent_span_id"], gateway.span_id.to_string());
    assert_eq!(records[2]["outcome"], "unknown");
    assert_eq!(records[2]["http_status"], 502);
    assert_eq!(records[2]["part_index"], 2);
    assert!(records[2]["duration_ms"].is_u64());
    for record in &records {
        assert_eq!(record["turn_id"], source.turn_id.unwrap().to_string());
        assert_eq!(
            record["trace_id"],
            source.turn_id.unwrap().simple().to_string()
        );
        assert_eq!(record["instance_id"], records[0]["instance_id"]);
    }
    if let Some(path) = std::env::var_os("CMAEG_TEST_TRACE_EXPORT") {
        let lines = records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>();
        std::fs::write(path, lines).unwrap();
    }
    assert!(
        lines.try_recv().is_err(),
        "finish must not emit a second end on drop"
    );
}

#[test]
fn cancelled_send_is_unknown_and_does_not_inherit_surrounding_fields() {
    let (output, lines) = capture();
    tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        let span = tracing::info_span!(
            "private",
            url = "private-url-canary",
            body = "private-body-canary"
        );
        let _entered = span.enter();
        drop(Operation::start(
            output.writer.clone(),
            Stage::Platform,
            Some(context()),
            None,
        ));
    });
    let start = read(&lines);
    let end = read(&lines);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(end["outcome"], "unknown");
    assert!(!end.to_string().contains("private"));
    assert_eq!(end["target"], "nyxid::turn_trace");
    assert_eq!(end["service"], "cma");
    assert_eq!(end["schema_version"], 1);
}

#[tokio::test]
async fn restarted_worker_uses_retained_parent_and_queued_send_keeps_original_turn() {
    use crate::telemetry::turn_trace::{scope, Association, AssociationSlot, BoundOperation};
    use tracing::instrument::WithSubscriber;
    use tracing::Instrument;
    use tracing_subscriber::layer::SubscriberExt;
    let (output, lines) = capture();
    let original = Association {
        turn_id: Uuid::new_v4(),
        parent_span_id: Some(SpanId::fresh()),
        ingress_request_id: Uuid::new_v4(),
    };
    let mut retained = AssociationSlot::default();
    retained.bind(original);
    let restored: AssociationSlot =
        serde_json::from_slice(&serde_json::to_vec(&retained).unwrap()).unwrap();
    async {
        let worker = BoundOperation::with_writer(
            output.writer.clone(),
            restored.get().unwrap(),
            Stage::Replay,
        );
        let queued = worker.within(async { tracing::Span::current() }).await;
        let context = scope::context(&queued).unwrap();
        assert_eq!(context.turn_id, Some(original.turn_id));
        assert_eq!(context.parent_span_id, original.parent_span_id);
        assert_eq!(
            context.ingress_request_id,
            Some(original.ingress_request_id)
        );
        assert_ne!(context.request_id, original.ingress_request_id);
        worker.finish(Outcome::Completed);
        let newer = BoundOperation::with_writer(
            output.writer.clone(),
            Association {
                turn_id: Uuid::new_v4(),
                ..original
            },
            Stage::Replay,
        );
        newer.finish(Outcome::Completed);
        async {
            let parent = scope::context(&tracing::Span::current()).unwrap();
            assert_eq!(parent.turn_id, Some(original.turn_id));
            let send = Operation::start(
                output.writer.clone(),
                Stage::Platform,
                Some(parent),
                Some(0),
            );
            send.finish(Outcome::Unknown, Some(502));
        }
        .instrument(queued)
        .await;
    }
    .with_subscriber(tracing_subscriber::registry().with(scope::testable_layer()))
    .await;
    let records: Vec<_> = (0..8).map(|_| read(&lines)).collect();
    assert_eq!(
        records[0]["parent_span_id"],
        original.parent_span_id.unwrap().to_string()
    );
    assert_eq!(records[1]["phase"], "link");
    assert_eq!(
        records[1]["ingress_request_id"],
        original.ingress_request_id.to_string()
    );
    assert_eq!(records[0]["span_id"], records[2]["span_id"]);
    assert_eq!(records[0]["request_id"], records[2]["request_id"]);
    assert_ne!(records[0]["span_id"], records[3]["span_id"]);
    assert_eq!(records[6]["parent_span_id"], records[0]["span_id"]);
    assert_eq!(records[7]["outcome"], "unknown");
    assert!(lines.try_recv().is_err());
}

struct Stalled {
    entered: mpsc::Sender<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl Write for Stalled {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.entered.send(());
        let (lock, condition) = &*self.release;
        let released = lock.lock().unwrap();
        drop(
            condition
                .wait_while(released, |released| !*released)
                .unwrap(),
        );
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn blocked_output_drops_records_and_never_blocks_trace_or_ordinary_log_producers() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let output = start(Stalled {
        entered: entered_tx,
        release: Arc::clone(&release),
    })
    .unwrap();
    let mut ordinary = output.logs.clone();
    ordinary.write_all(b"ordinary service log\n").unwrap();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let writer = output.writer.clone();
    let producer = thread::spawn(move || {
        for _ in 0..8192 {
            drop(Operation::start(writer.clone(), Stage::Gateway, None, None));
            ordinary.write_all(b"ordinary service log\n").unwrap();
        }
        done_tx.send(()).unwrap();
    });
    let completed = done_rx.recv_timeout(Duration::from_secs(2));
    let drops = output.writer.dropped();
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    producer.join().unwrap();
    assert!(completed.is_ok(), "a slow sink blocked a log producer");
    assert!(drops > 0, "overflow must be counted");
}

struct Broken;
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_io_failure_is_counted_without_exposing_an_error_payload() {
    let output = start(Broken).unwrap();
    drop(Operation::start(
        output.writer.clone(),
        Stage::Gateway,
        None,
        None,
    ));
    let deadline = Instant::now() + Duration::from_secs(2);
    while output.writer.dropped() < 2 && Instant::now() < deadline {
        thread::yield_now();
    }
    assert_eq!(output.writer.dropped(), 2);
}

#[test]
fn idle_writer_does_not_hold_stdout_lock() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "telemetry::turn_trace::writer::tests::idle_stdout_probe",
            "--nocapture",
        ])
        .env("CMAEG_TRACE_STDOUT_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stdout probe exited {:?}",
        result.status.code()
    );
}

#[test]
fn idle_stdout_probe() {
    if std::env::var_os("CMAEG_TRACE_STDOUT_PROBE").is_none() {
        return;
    }
    struct Signal(mpsc::Sender<()>);
    impl Write for Signal {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            std::io::stdout().write_all(bytes)?;
            let _ = self.0.send(());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            std::io::stdout().flush()
        }
    }
    let (written_tx, written_rx) = mpsc::channel();
    let output = start(Signal(written_tx)).unwrap();
    drop(Operation::start(
        output.writer.clone(),
        Stage::Gateway,
        None,
        None,
    ));
    if (0..2).any(|_| written_rx.recv_timeout(Duration::from_secs(2)).is_err()) {
        std::process::exit(41);
    }
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(b"ordinary stdout remains available\n")
            .unwrap();
        let _ = done_tx.send(());
    });
    // Exit directly on failure: the Rust harness also needs stdout to report.
    std::process::exit(if done_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
        0
    } else {
        42
    });
}

#[test]
fn oversized_ordinary_logs_are_dropped_before_entering_the_shared_queue() {
    let (mut output, records) = capture();
    let oversized = vec![b'x'; super::super::record::MAX_RECORD_BYTES + 1];
    output.logs.write_all(&oversized).unwrap();
    assert_eq!(output.writer.dropped(), 1);
    assert!(records.try_recv().is_err());
    let operation = Operation::start(output.writer.clone(), Stage::Gateway, None, None);
    operation.finish(Outcome::Completed, Some(200));
    let start = read(&records);
    let end = read(&records);
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(output.writer.dropped(), 1);
    assert!(records.try_recv().is_err());
}
