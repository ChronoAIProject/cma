use super::{fixture::Capture, record::Record, *};
use layer2_core::diagnostic_context::DiagnosticContext;

fn metadata() -> Metadata {
    Metadata::new("general", DiagnosticContext::background("fixture"))
}

#[test]
fn only_exact_true_enables_diagnostics() {
    for value in [None, Some(""), Some("false"), Some("TRUE"), Some(" true ")] {
        assert!(!enabled_value(value));
    }
    assert!(enabled_value(Some("true")));
}

#[tokio::test(start_paused = true)]
async fn ring_expires_old_rows_but_retains_outstanding_holds_and_freezes_immutable_snapshot() {
    let capture = Capture::new();
    let mut row = metadata();
    row.hold_hash = "a".repeat(64).into();
    capture
        .recorder
        .observe(row.clone(), "checked_out", Duration::ZERO);
    tokio::time::advance(Duration::from_secs(31)).await;
    capture
        .recorder
        .observe(row.clone(), "command_started", Duration::ZERO);
    capture.recorder.freeze(1);
    capture
        .recorder
        .observe(row, "checked_in", Duration::from_secs(31));
    let frozen = capture.snapshots();
    assert_eq!(frozen.len(), 1);
    assert_eq!(frozen[0].rows.len(), 1);
    assert_eq!(frozen[0].active.len(), 2);
    assert!(frozen[0].active.iter().any(|r| r.event == "checked_out"));
    capture.recorder.freeze(2);
    assert!(capture.snapshots()[0].active.is_empty());
}

#[tokio::test(start_paused = true)]
async fn ring_capacity_and_full_snapshot_lane_are_bounded_without_waiting() {
    let capture = Capture::new();
    for _ in 0..ring::CAPACITY + 5 {
        capture
            .recorder
            .observe(metadata(), "pool_ready", Duration::ZERO);
    }
    for check in 1..=10 {
        capture.recorder.freeze(check);
    }
    let snapshots = capture.snapshots();
    assert_eq!(snapshots.len(), output::SNAPSHOT_CAPACITY);
    assert_eq!(snapshots[0].rows.len(), ring::CAPACITY);
    assert_eq!(snapshots[0].evicted, 5);
    assert_eq!(capture.recorder.output.loss_count(), 8);
}

#[tokio::test(start_paused = true)]
async fn busy_ring_and_full_or_disconnected_output_report_loss_without_blocking() {
    let capture = Capture::new();
    let guard = capture.recorder.ring.lock().unwrap();
    capture
        .recorder
        .observe(metadata(), "pool_ready", Duration::ZERO);
    capture.recorder.freeze(1);
    drop(guard);
    assert_eq!(capture.recorder.loss.load(Ordering::Relaxed), 2);
    for _ in 0..output::RECORD_CAPACITY + 1 {
        capture.recorder.response(0, "pending", None, 0);
    }
    assert_eq!(capture.recorder.output.loss_count(), 1);
    let recorder = capture.recorder.clone();
    drop(capture);
    recorder.response(0, "pending", None, 0);
    assert_eq!(recorder.output.loss_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn attempt_records_wake_lag_overshoot_and_cancellation_phase() {
    let capture = Capture::new();
    let scheduled = Instant::now();
    tokio::time::advance(Duration::from_millis(75)).await;
    let mut attempt = Attempt::start(
        Some(capture.recorder.clone()),
        scheduled,
        Duration::from_secs(2),
    );
    attempt
        .run(async {
            Progress::current().unwrap().event("checkout_started", 7);
        })
        .await;
    tokio::time::advance(Duration::from_millis(2200)).await;
    attempt.finish("deadline");
    let records = capture.records();
    assert!(records.iter().any(|r| matches!(
        r,
        Record::Attempt {
            event: "completed",
            outcome: "deadline",
            phase: "checkout",
            wake_lag_us: 75_000,
            overshoot_us: 200_000,
            operation: 7,
            ..
        }
    )));
    assert_eq!(capture.snapshots().len(), 1);
    let check = attempt.check();
    drop(attempt);
    assert!(
        capture.records().is_empty(),
        "finished attempt must not emit cancellation"
    );
    let cancelled = Attempt::start(
        Some(capture.recorder.clone()),
        Instant::now(),
        Duration::from_secs(2),
    );
    assert_ne!(check, cancelled.check());
    cancelled
        .run(async {
            Progress::current().unwrap().event("command_started", 9);
        })
        .await;
    drop(cancelled);
    assert!(capture.records().iter().any(|r| matches!(
        r,
        Record::Attempt {
            outcome: "cancelled",
            phase: "command",
            operation: 9,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn actual_event_times_and_context_remain_private_and_survive_writer_delay() {
    let capture = Capture::new();
    let mut context = DiagnosticContext::background("health_monitor");
    context.route = "GET /api/v1/sandboxes/{id}".into();
    context.request_hash = "do-not-retain-this-value".into();
    capture.recorder.observe(
        Metadata::new("general", context),
        "checkout_started",
        Duration::ZERO,
    );
    let mut attempt = Attempt::start(
        Some(capture.recorder.clone()),
        Instant::now(),
        Duration::from_secs(2),
    );
    tokio::time::advance(Duration::from_secs(2)).await;
    attempt.finish("deadline");
    tokio::time::advance(Duration::from_secs(5)).await;
    let logs = capture.logs();
    assert!(!logs.contains("do-not-retain-this-value"));
    let rows: Vec<serde_json::Value> = logs
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let mongo = rows
        .iter()
        .find(|r| r["fields"]["message"] == "Readiness Mongo metadata")
        .unwrap();
    assert_eq!(mongo["fields"]["process_monotonic_us"], 0);
    assert_eq!(mongo["fields"]["route"], "GET /api/v1/sandboxes/{id}");
    assert!(mongo.get("span").is_none());
    assert!(rows.iter().any(|r| r["fields"]["outcome"] == "deadline"));
}

#[tokio::test(start_paused = true)]
async fn unrelated_attempts_do_not_share_progress() {
    let capture = Capture::new();
    let a = Attempt::start(
        Some(capture.recorder.clone()),
        Instant::now(),
        Duration::from_secs(2),
    );
    let b = Attempt::start(
        Some(capture.recorder.clone()),
        Instant::now(),
        Duration::from_secs(2),
    );
    let (one, two) = tokio::join!(
        a.run(async {
            tokio::task::yield_now().await;
            Progress::current().unwrap().check
        }),
        b.run(async { Progress::current().unwrap().check })
    );
    assert_eq!((one, two), (a.check(), b.check()));
    assert!(Progress::current().is_none());
    let off = Attempt::start(None, Instant::now(), Duration::from_secs(2));
    off.run(async {
        assert!(Progress::current().is_none());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn shared_timer_records_late_wakeup_and_stops_when_aborted() {
    let capture = Capture::new();
    let timer = tokio::spawn(timer::run(Some(capture.recorder.clone()), |_, _| {}));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1500)).await;
    tokio::task::yield_now().await;
    assert!(capture.records().iter().any(|r| matches!(
        r,
        Record::Timer {
            lag_us: 500_000,
            ..
        }
    )));
    timer.abort();
    assert!(timer.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert!(capture.records().is_empty());
}

#[tokio::test(start_paused = true)]
async fn post_deadline_metadata_cannot_consume_the_attempt_record_lane() {
    let capture = Capture::new();
    capture.recorder.freeze(1);
    for _ in 0..output::RECORD_CAPACITY + 20 {
        capture
            .recorder
            .observe(metadata(), "pool_ready", Duration::ZERO);
    }
    let mut attempt = Attempt::start(
        Some(capture.recorder.clone()),
        Instant::now(),
        Duration::from_secs(2),
    );
    attempt.finish("ready");
    assert!(capture.records().iter().any(|r| matches!(
        r,
        Record::Attempt {
            outcome: "ready",
            ..
        }
    )));
    assert_eq!(capture.recorder.output.loss_count(), 20);
}
