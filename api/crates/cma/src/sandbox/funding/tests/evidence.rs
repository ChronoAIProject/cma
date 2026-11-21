use super::*;

#[tokio::test]
async fn evidence_is_frozen_before_send_and_cursor_survives_restart() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.fund(370).await.unwrap();
    *h.state.events.lock().unwrap() = vec![
        event(&h.sandbox, 1, 90, 350),
        event(&h.sandbox, 2, 350, 450),
    ];
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 400).await.unwrap_err(); // second interval is still in the future
    assert!(h.provider.evidence.lock().unwrap().is_empty());
    h.provider.lose_evidence.store(true, Ordering::SeqCst);
    assert!(h.adapter.emit_evidence(&mut state, 450).await.is_err());
    assert_eq!(h.saved().await.evidence_cursor, 0);
    let frozen = h.saved().await.windows[0].evidence.clone().unwrap();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 3, 450, 700));
    let restarted =
        ProviderFundingAdapter::new(h.provider.clone(), h.state.clone(), h.control.clone());
    let mut recovered = h.saved().await;
    restarted.emit_evidence(&mut recovered, 700).await.unwrap();
    assert!(h.saved().await.windows.is_empty());
    assert_eq!(h.saved().await.evidence_cursor, 3);
    let sent = h.provider.evidence.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0], frozen);
    assert_eq!(sent[0], sent[1]);
    assert_eq!(
        sent[2].intervals[0], sent[0].intervals[1],
        "boundary revision must be identical"
    );
}
#[tokio::test]
async fn partial_window_delivery_advances_only_acknowledged_cursor() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.fund(370).await.unwrap();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 700).await.unwrap();
    assert_eq!(state.windows.len(), 1);
    assert_eq!(state.evidence_cursor, 1);
    assert_eq!(h.provider.evidence.lock().unwrap().len(), 1);
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 2, 400, 700));
    h.adapter.emit_evidence(&mut state, 700).await.unwrap();
    assert!(state.windows.is_empty());
    assert_eq!(state.evidence_cursor, 2);
}
#[tokio::test]
async fn evidence_failure_does_not_discard_newly_confirmed_coverage() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.fund(370).await.unwrap();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    h.provider.lose_evidence.store(true, Ordering::SeqCst);
    assert_eq!(h.adapter.renew_due(at(401)).await.unwrap(), 1);
    assert_eq!(h.saved().await.current.unwrap().covered_until, 700);
    assert_eq!(h.saved().await.windows.len(), 2);
}
#[tokio::test]
async fn paused_retained_disk_renews_without_resurrecting_compute() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.adapter.settle(&h.sandbox, at(120)).await.unwrap();
    let mut paused = h.sandbox.clone();
    paused.runtime = None;
    paused.status = layer2_core::sandbox::SandboxStatus::Paused;
    *h.state.sandbox.lock().unwrap() = Some(paused);
    assert_eq!(h.adapter.renew_due(at(370)).await.unwrap(), 1);
    let state = h.saved().await;
    let receipt = state.current.unwrap();
    assert_eq!(receipt.allocation.cpu_millis, 0);
    assert_eq!(receipt.allocation.memory_mib, 0);
    assert_eq!(receipt.allocation.disk_gib, 64);
    assert!(state.stopped);
    assert_eq!(h.control.deadlines.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn purged_sandbox_requires_actual_retained_proof() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    *h.state.sandbox.lock().unwrap() = None;
    assert_eq!(h.adapter.renew_due(at(401)).await.unwrap(), 1);
    assert_eq!(h.saved().await.windows.len(), 1);
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    h.adapter.renew_due(at(462)).await.unwrap();
    assert!(h.saved().await.windows.is_empty());
}
#[tokio::test]
async fn malformed_evidence_and_receipts_are_rejected_without_send() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    let w = h.saved().await.current.unwrap();
    let good = event(&h.sandbox, 1, 100, 400);
    assert!(
        super::super::evidence::intervals(std::slice::from_ref(&good), 0, &w, 400)
            .unwrap()
            .is_some()
    );
    let mut invalid = Vec::new();
    let mut e = good.clone();
    e.owner = layer2_core::SubjectId("another".into());
    invalid.push(e);
    let mut e = good.clone();
    e.sandbox_id = SandboxId("another".into());
    invalid.push(e);
    let mut e = good.clone();
    e.revision = 2;
    invalid.push(e);
    let mut e = good.clone();
    e.quantity.cpu_millis_seconds += 1;
    invalid.push(e);
    let mut e = good.clone();
    e.started_at = 401;
    invalid.push(e);
    let mut e = good.clone();
    e.started_at = 101;
    e.quantity = e.allocation.usage(299).unwrap();
    invalid.push(e);
    let mut e = good.clone();
    e.allocation.cpu_millis += 1;
    e.quantity = e.allocation.usage(300).unwrap();
    invalid.push(e);
    for e in invalid {
        assert!(super::super::evidence::intervals(&[e], 0, &w, 400).is_err());
    }
    assert!(super::super::evidence::intervals(&[good], u64::MAX, &w, 400).is_err());
    let gap = vec![
        event(&h.sandbox, 1, 100, 200),
        event(&h.sandbox, 2, 201, 400),
    ];
    assert!(super::super::evidence::intervals(&gap, 0, &w, 400).is_err());
    let many: Vec<_> = (1..=1025).map(|r| event(&h.sandbox, r, 100, 100)).collect();
    assert!(super::super::evidence::intervals(&many, 0, &w, 400).is_err());
    let mut request = h.provider.requests.lock().unwrap()[0].clone();
    request.seconds = 0;
    assert!(validate_receipt(&request, &w, None).is_err());
    request.seconds = 300;
    for changed in [
        "owner",
        "quote",
        "generation",
        "allocation",
        "duration",
        "identity",
    ] {
        let mut wrong = w.clone();
        match changed {
            "owner" => wrong.owner_id = "wrong".into(),
            "quote" => wrong.quote_id = "wrong".into(),
            "generation" => wrong.runtime_generation += 1,
            "allocation" => wrong.allocation.disk_gib += 1,
            "duration" => wrong.covered_until = wrong.covered_from,
            "identity" => wrong.request_id = "wrong".into(),
            _ => unreachable!(),
        }
        assert!(validate_receipt(&request, &wrong, None).is_err());
    }
    assert!(h.provider.evidence.lock().unwrap().is_empty());
}

#[tokio::test]
async fn historical_scan_is_durable_and_never_exports_unfunded_intervals() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    *h.state.events.lock().unwrap() =
        vec![event(&h.sandbox, 1, 10, 20), event(&h.sandbox, 2, 20, 90)];
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 401).await.unwrap();
    assert_eq!(h.saved().await.evidence_scan_cursor, 2);
    assert_eq!(state.evidence_cursor, 0);
    assert!(h.provider.evidence.lock().unwrap().is_empty());
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 3, 90, 400));
    let mut recovered = h.saved().await;
    h.adapter.emit_evidence(&mut recovered, 401).await.unwrap();
    assert_eq!(recovered.evidence_cursor, 3);
    assert_eq!(h.provider.evidence.lock().unwrap()[0].intervals.len(), 1);
}
