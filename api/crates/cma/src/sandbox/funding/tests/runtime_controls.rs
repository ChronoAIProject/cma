use super::*;

pub(super) fn replacement(sandbox: &Sandbox) -> Sandbox {
    let mut next = sandbox.clone();
    next.lifecycle.runtime_generation_high_water = 2;
    let binding = next.runtime.as_mut().unwrap();
    binding.provision_generation = Some(2);
    binding.runtime_id = "runtime-replacement".into();
    next.revision += 1;
    next
}

#[tokio::test]
async fn retained_coverage_cannot_install_a_deadline_on_a_replacement_runtime() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    let original = h.saved().await;
    *h.state.sandbox.lock().unwrap() = Some(replacement(&h.sandbox));
    h.control.deadlines.lock().unwrap().clear();

    assert_eq!(h.adapter.renew_due(at(110)).await.unwrap(), 1);

    assert!(h.control.deadlines.lock().unwrap().is_empty());
    assert!(h.control.pauses.lock().unwrap().is_empty());
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert_eq!(h.saved().await.current, original.current);
    assert_eq!(h.saved().await.windows, original.windows);
}

#[tokio::test]
async fn old_reservation_recovery_failure_cannot_pause_a_replacement_runtime() {
    let h = Harness::new();
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    let pending = h.saved().await.pending.unwrap();
    *h.state.sandbox.lock().unwrap() = Some(replacement(&h.sandbox));
    h.control.pauses.lock().unwrap().clear();
    h.provider.fail_recover.store(true, Ordering::SeqCst);

    assert_eq!(h.adapter.renew_due(at(110)).await.unwrap(), 0);

    assert!(h.control.pauses.lock().unwrap().is_empty());
    assert!(h.control.deadlines.lock().unwrap().is_empty());
    let failed = h.saved().await;
    assert_eq!(failed.pending.as_ref(), Some(&pending));
    assert!(failed.retry_at > 110 && failed.last_error.is_some());
    h.provider.fail_recover.store(false, Ordering::SeqCst);
    assert_eq!(h.adapter.renew_due(at(failed.retry_at)).await.unwrap(), 1);
    assert!(h.saved().await.pending.is_none());
    assert_eq!(
        h.saved().await.current.unwrap().request_id,
        pending.request_id
    );
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert!(h.control.deadlines.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stale_stop_callback_cannot_mark_new_generation_funding_stopped() {
    let mut h = Harness::new();
    h.fund(100).await.unwrap();
    let old = h.sandbox.clone();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&old, 1, 100, 400));
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 401).await.unwrap();
    h.sandbox = replacement(&old);
    h.provider.now.store(401, Ordering::SeqCst);
    assert_eq!(h.fund(401).await.unwrap(), Some(701));
    let admitted = h.saved().await;

    h.adapter.settle(&old, at(410)).await.unwrap();

    assert_eq!(h.saved().await, admitted);
    assert_eq!(h.fund(411).await.unwrap(), Some(701));
}

#[tokio::test]
async fn stop_racing_new_generation_conflicts_then_becomes_a_no_write_retry() {
    let mut h = Harness::new();
    h.fund(100).await.unwrap();
    let old = h.sandbox.clone();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&old, 1, 100, 400));
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 401).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    *h.state.load_barrier.lock().unwrap() = Some(barrier.clone());
    let adapter = h.adapter.clone();
    let completion = old.clone();
    let stopped = tokio::spawn(async move { adapter.settle(&completion, at(410)).await });
    barrier.wait().await;
    h.sandbox = replacement(&old);
    h.provider.now.store(401, Ordering::SeqCst);
    h.fund(401).await.unwrap();
    let admitted = h.saved().await;
    barrier.wait().await;
    assert_eq!(stopped.await.unwrap(), Err(FundingError::Conflict));
    h.adapter.settle(&old, at(411)).await.unwrap();
    assert_eq!(h.saved().await, admitted);
}

#[tokio::test]
async fn replacement_does_not_discard_original_frozen_evidence_or_invent_a_new_reservation() {
    let mut h = Harness::new();
    h.fund(100).await.unwrap();
    let original = h.saved().await.current.unwrap();
    *h.state.sandbox.lock().unwrap() = Some(replacement(&h.sandbox));
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    h.provider.lose_evidence.store(true, Ordering::SeqCst);
    h.control.deadlines.lock().unwrap().clear();
    assert_eq!(h.adapter.renew_due(at(401)).await.unwrap(), 1);
    let first = h.provider.evidence.lock().unwrap()[0].clone();
    assert_eq!(first.window_id, original.window_id);
    assert_eq!(first.runtime_generation, 1);
    assert_eq!(h.saved().await.windows[0].evidence, Some(first.clone()));
    h.adapter = ProviderFundingAdapter::new(h.provider.clone(), h.state.clone(), h.control.clone());
    assert_eq!(h.adapter.renew_due(at(406)).await.unwrap(), 1);
    assert_eq!(
        *h.provider.evidence.lock().unwrap(),
        vec![first.clone(), first]
    );
    assert!(h.saved().await.windows.is_empty());
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert!(h.control.deadlines.lock().unwrap().is_empty());
    assert!(h.control.pauses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn future_or_ambiguous_completion_cannot_stop_retained_funding() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    let before = h.saved().await;
    assert_eq!(
        h.adapter.settle(&replacement(&h.sandbox), at(110)).await,
        Err(FundingError::Conflict)
    );
    let mut ambiguous = h.sandbox.clone();
    ambiguous.lifecycle.runtime_provision =
        Some(layer2_core::sandbox::SandboxRuntimeProvisionReservation {
            operation_id: "fixture-replacement".into(),
            generation: 2,
        });
    assert_eq!(
        h.adapter.settle(&ambiguous, at(110)).await,
        Err(FundingError::Conflict)
    );
    assert_eq!(h.saved().await, before);

    ambiguous.runtime.as_mut().unwrap().provision_generation = Some(1);
    h.adapter.settle(&ambiguous, at(111)).await.unwrap();
    assert!(h.saved().await.stopped);
}

#[tokio::test]
async fn prospective_generation_never_authorizes_deadline_on_an_old_bound_runtime() {
    let h = Harness::new();
    let mut mixed = h.sandbox.clone();
    mixed.runtime.as_mut().unwrap().provision_generation = Some(1);
    mixed.lifecycle.runtime_provision =
        Some(layer2_core::sandbox::SandboxRuntimeProvisionReservation {
            operation_id: "fixture-replacement".into(),
            generation: 2,
        });
    assert_eq!(
        h.adapter.ensure_coverage(&mixed, at(100)).await,
        Err(FundingError::Conflict)
    );
    assert!(h.control.deadlines.lock().unwrap().is_empty());
    assert_eq!(h.saved().await.current.unwrap().runtime_generation, 2);
}
