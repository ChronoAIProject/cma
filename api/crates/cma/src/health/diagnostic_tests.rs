use super::{
    mongo_fixture::{DriverProbe, MongoPeer},
    tests::response,
    *,
};
use crate::readiness_diagnostics::{fixture::Capture, record::Record};

async fn deadline(app: &Router) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if response(app, "/ready").await.1["dependency"]["reason"] == "deadline" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn diagnostic_unanswered_driver_ping_joins_deadline_command_and_cached_response() {
    let capture = Capture::new();
    let peer = MongoPeer::start_with_diagnostics(Some(capture.recorder.clone())).await;
    peer.block(true);
    let app = router_with_diagnostics(
        Arc::new(DriverProbe(peer.client.clone())),
        Some(capture.recorder.clone()),
    );
    peer.wait_for(|| peer.ping_count() == 2).await;
    deadline(&app).await;
    let records = capture.records();
    let check = records
        .iter()
        .find_map(|r| match r {
            Record::Attempt {
                check,
                outcome: "deadline",
                phase: "command",
                ..
            } => Some(*check),
            _ => None,
        })
        .expect("real driver command phase must survive timeout cancellation");
    assert!(records.iter().any(|r| matches!(r, Record::Response { check: id, reason: "deadline", age_known: true, .. } if *id == check)));
    let snapshots = capture.snapshots();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].check, check);
    assert!(snapshots[0]
        .rows
        .iter()
        .any(|r| r.event == "command_started"
            && r.metadata.check == check
            && r.metadata.command == "ping"
            && r.metadata.hold_hash.as_ref() != "none"));
    peer.wait_for(|| peer.in_use() == 0).await;
    peer.block(false);
    drop(app);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn diagnostic_saturated_driver_pool_distinguishes_checkout_from_command() {
    let capture = Capture::new();
    let peer = MongoPeer::start_with_diagnostics(Some(capture.recorder.clone())).await;
    peer.block(true);
    let client = peer.client.clone();
    let holder = tokio::spawn(async move { DriverProbe(client).ping().await });
    peer.wait_for(|| peer.ping_count() == 2).await;
    let app = router_with_diagnostics(
        Arc::new(DriverProbe(peer.client.clone())),
        Some(capture.recorder.clone()),
    );
    peer.wait_for(|| peer.trace.lock().unwrap().checkout_started == 3)
        .await;
    deadline(&app).await;
    assert_eq!(peer.ping_count(), 2);
    assert!(capture.records().iter().any(|r| matches!(
        r,
        Record::Attempt {
            outcome: "deadline",
            phase: "checkout",
            ..
        }
    )));
    let snapshots = capture.snapshots();
    assert!(snapshots[0]
        .active
        .iter()
        .any(|r| r.metadata.check == 0 && r.event == "checked_out"));
    assert!(snapshots[0]
        .rows
        .iter()
        .any(|r| r.metadata.check != 0 && r.event == "checkout_started"));
    peer.block(false);
    holder.await.unwrap().unwrap();
    drop(app);
    peer.client.clone().shutdown().await;
}

#[tokio::test]
async fn diagnostic_router_drop_records_cancelled_phase_and_releases_connection() {
    let capture = Capture::new();
    let peer = MongoPeer::start_with_diagnostics(Some(capture.recorder.clone())).await;
    peer.block(true);
    let app = router_with_diagnostics(
        Arc::new(DriverProbe(peer.client.clone())),
        Some(capture.recorder.clone()),
    );
    peer.wait_for(|| peer.ping_count() == 2).await;
    drop(app);
    peer.wait_for(|| peer.in_use() == 0).await;
    assert!(capture.records().iter().any(|r| matches!(
        r,
        Record::Attempt {
            outcome: "cancelled",
            phase: "command",
            ..
        }
    )));
    assert!(capture.snapshots().is_empty());
    peer.client.clone().shutdown().await;
}

struct Immediate(bool);
#[async_trait::async_trait]
impl ReadinessProbe for Immediate {
    async fn ping(&self) -> Result<(), StoreError> {
        if self.0 {
            Ok(())
        } else {
            Err(StoreError::Unavailable("do-not-retain-error-detail".into()))
        }
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_worker_refusal_preserves_dependency_reason_and_recovery() {
    for store_available in [true, false] {
        let capture = Capture::new();
        let workers = crate::worker_supervision::Workers::new();
        let app = monitored(
            Arc::new(Immediate(store_available)),
            Some(workers.clone()),
            false,
            Some(capture.recorder.clone()),
            None,
        );
        tokio::task::yield_now().await;
        let (status, body) = response(&app, "/ready").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body["reason"],
            if store_available { "workers" } else { "store" }
        );
        let records = capture.records();
        let check = records
            .iter()
            .find_map(|record| match record {
                Record::Attempt {
                    check,
                    event: "completed",
                    ..
                } => Some(*check),
                _ => None,
            })
            .unwrap();
        assert!(records.iter().any(|record| matches!(record,
            Record::Response { check: id, reason, age_known: true, .. }
                if *id == check && *reason == if store_available { "workers" } else { "unavailable" }
        )));
        assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);

        for worker in workers.snapshot().iter().filter(|worker| worker.required) {
            let activity = workers.get(worker.worker);
            activity.scheduled();
            activity.waiting(Duration::from_secs(1), "idle_scan");
        }
        let (status, body) = response(&app, "/ready").await;
        if store_available {
            assert_eq!(status, StatusCode::OK);
            assert!(body["reason"].is_null());
            assert!(!capture
                .records()
                .iter()
                .any(|record| matches!(record, Record::Response { .. })));
        } else {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["reason"], "store");
            assert!(capture.records().iter().any(|record| matches!(record,
                Record::Response { check: id, reason: "unavailable", .. } if *id == check
            )));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_success_and_repeated_error_keep_existing_response_and_cadence() {
    for success in [true, false] {
        let capture = Capture::new();
        let app =
            router_with_diagnostics(Arc::new(Immediate(success)), Some(capture.recorder.clone()));
        tokio::task::yield_now().await;
        let first = response(&app, "/ready").await;
        assert_eq!(
            first.0,
            if success {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        assert!(
            first.1.get("check_sequence").is_none(),
            "diagnostics do not change the public response"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let records = capture.records();
        let completed: Vec<_> = records
            .iter()
            .filter(|r| {
                matches!(
                    r,
                    Record::Attempt {
                        event: "completed",
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            completed.len(),
            2,
            "repeat failures are retained even without a state transition"
        );
        assert!(completed.iter().all(|r| matches!(r, Record::Attempt { outcome, .. } if *outcome == if success {"ready"} else {"unavailable"})));
        assert!(capture.snapshots().is_empty());
        drop(app);
    }
}

#[tokio::test(start_paused = true)]
async fn diagnostic_pending_and_stale_responses_are_distinct() {
    let capture = Capture::new();
    let (sender, state) = watch::channel(None);
    let app = routes(state, Some(capture.recorder.clone()));
    assert_eq!(
        response(&app, "/ready").await.1["dependency"]["reason"],
        "pending"
    );
    sender.send_replace(Some(CompletedCheck {
        completed: Instant::now(),
        checked_at: "2026-11-12T00:00:00Z".into(),
        duration_ms: 10,
        failure: None,
        diagnostic_check: 27,
    }));
    tokio::time::advance(MAX_CHECK_AGE).await;
    assert_eq!(
        response(&app, "/ready").await.1["dependency"]["reason"],
        "stale"
    );
    let records = capture.records();
    assert!(records.iter().any(|r| matches!(
        r,
        Record::Response {
            check: 0,
            reason: "pending",
            age_known: false,
            ..
        }
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        Record::Response {
            check: 27,
            reason: "stale",
            age_us: 5_000_000,
            ..
        }
    )));
}
