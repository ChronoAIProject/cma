use super::*;

pub(super) async fn pending() -> Harness {
    let h = Harness::new();
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    *h.state.sandbox.lock().unwrap() = None;
    h.control.pauses.lock().unwrap().clear();
    h.control.deadlines.lock().unwrap().clear();
    h
}

pub(super) fn no_controls(h: &Harness) {
    assert!(h.control.pauses.lock().unwrap().is_empty());
    assert!(h.control.deadlines.lock().unwrap().is_empty());
}

#[tokio::test]
async fn purged_sandbox_recovers_committed_pending_receipt_without_new_admission() {
    let h = pending().await;
    let original = h.saved().await;
    let pending = original.pending.clone().unwrap();
    let receipt = h.provider.receipts.lock().unwrap()[&pending.request_id].clone();
    assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 1);
    let recovered = h.saved().await;
    assert!(recovered.pending.is_none());
    assert_eq!(recovered.current.as_ref(), Some(&receipt));
    assert_eq!(recovered.windows.len(), 1);
    assert_eq!(recovered.windows[0].window, receipt);
    assert!(recovered.windows[0].evidence.is_none());
    assert_eq!(recovered.operation_id, original.operation_id);
    assert_eq!(recovered.runtime_generation, original.runtime_generation);
    assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 1);
    assert_eq!(h.provider.requests.lock().unwrap().as_slice(), &[pending]);
    assert!(h.provider.evidence.lock().unwrap().is_empty());
    no_controls(&h);
    assert_eq!(recovered.retry_at, 560);
    h.adapter.renew_due(at(561)).await.unwrap();
    assert_eq!(h.saved().await.windows.len(), 1);
    assert_eq!(h.provider.recoveries.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn missing_receipt_and_lookup_failure_preserve_unknown_intent_and_retry_gate() {
    for outage in [false, true] {
        let h = pending().await;
        let original = h.saved().await;
        if outage {
            h.provider.fail_recover.store(true, Ordering::SeqCst);
        } else {
            h.provider.receipts.lock().unwrap().clear();
        }
        assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 0);
        let saved = h.saved().await;
        assert_eq!(saved.pending, original.pending);
        assert_eq!(saved.current, original.current);
        assert_eq!(saved.windows, original.windows);
        assert_eq!(saved.quote, original.quote);
        assert_eq!(saved.operation_id, original.operation_id);
        assert_eq!(saved.retry_at, 560);
        assert_eq!(saved.attempts, original.attempts + 1);
        assert_eq!(h.adapter.renew_due(at(559)).await.unwrap(), 0);
        assert_eq!(h.provider.recoveries.lock().unwrap().len(), 2);
        assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
        assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 1);
        assert!(h.provider.evidence.lock().unwrap().is_empty());
        no_controls(&h);
    }
}

#[tokio::test]
async fn missing_recovery_rejects_local_identity_and_predecessor_conflicts_before_lookup() {
    for field in [
        "owner",
        "sandbox",
        "generation",
        "operation",
        "quote",
        "predecessor",
    ] {
        let h = pending().await;
        let mut original = h.saved().await;
        let request = original.pending.as_mut().unwrap();
        match field {
            "owner" => request.owner_id = "foreign".into(),
            "sandbox" => request.sandbox_id = "foreign".into(),
            "generation" => request.runtime_generation += 1,
            "operation" => request.operation_id = "foreign".into(),
            "quote" => request.quote_id = "foreign".into(),
            "predecessor" => request.previous_window_id = Some("foreign".into()),
            _ => unreachable!(),
        }
        h.state.save(&mut original).await.unwrap();
        assert!(
            matches!(
                h.adapter.reconcile(original.clone(), at(500)).await,
                Err(FundingError::Invalid(_))
            ),
            "{field}"
        );
        let saved = h.saved().await;
        assert_eq!(saved.pending, original.pending, "{field}");
        assert_eq!(saved.current, original.current, "{field}");
        assert_eq!(saved.windows, original.windows, "{field}");
        assert_eq!(h.provider.recoveries.lock().unwrap().len(), 1, "{field}");
        assert!(h.provider.evidence.lock().unwrap().is_empty());
        no_controls(&h);
    }
}

#[tokio::test]
async fn missing_recovery_rejects_foreign_receipts_without_adopting_or_sending() {
    for field in [
        "request",
        "quote",
        "owner",
        "sandbox",
        "generation",
        "allocation",
        "coverage",
    ] {
        let h = pending().await;
        let original = h.saved().await;
        {
            let mut receipts = h.provider.receipts.lock().unwrap();
            let receipt = receipts
                .get_mut(&original.pending.as_ref().unwrap().request_id)
                .unwrap();
            match field {
                "request" => receipt.request_id = "foreign".into(),
                "quote" => receipt.quote_id = "foreign".into(),
                "owner" => receipt.owner_id = "foreign".into(),
                "sandbox" => receipt.sandbox_id = "foreign".into(),
                "generation" => receipt.runtime_generation += 1,
                "allocation" => receipt.allocation.cpu_millis += 1,
                "coverage" => receipt.covered_until = receipt.covered_from,
                _ => unreachable!(),
            }
        }
        assert!(
            matches!(
                h.adapter.reconcile(original.clone(), at(500)).await,
                Err(FundingError::Invalid(_))
            ),
            "{field}"
        );
        let saved = h.saved().await;
        assert_eq!(saved.pending, original.pending);
        assert_eq!(saved.current, original.current);
        assert!(saved.windows.is_empty());
        assert!(h.provider.evidence.lock().unwrap().is_empty());
        assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
        no_controls(&h);
    }
}

#[tokio::test]
async fn failed_or_uncertain_adoption_never_sends_unconfirmed_state_and_restart_is_idempotent() {
    for committed in [false, true] {
        let h = pending().await;
        h.state
            .events
            .lock()
            .unwrap()
            .push(event(&h.sandbox, 1, 100, 400));
        if committed {
            h.state.lose_adoption_ack.store(true, Ordering::SeqCst);
        } else {
            h.state.fail_adoption.store(true, Ordering::SeqCst);
        }
        assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 0);
        let saved = h.saved().await;
        assert_eq!(saved.pending.is_none(), committed);
        assert_eq!(saved.windows.len(), usize::from(committed));
        assert!(h.provider.evidence.lock().unwrap().is_empty());
        let restarted =
            ProviderFundingAdapter::new(h.provider.clone(), h.state.clone(), h.control.clone());
        restarted.renew_due(at(561)).await.unwrap();
        restarted.renew_due(at(622)).await.unwrap();
        assert!(h.saved().await.pending.is_none());
        assert!(h.saved().await.windows.is_empty());
        assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
        assert_eq!(h.provider.evidence.lock().unwrap().len(), 1);
        no_controls(&h);
    }
}

#[tokio::test]
async fn old_frozen_evidence_progresses_during_lookup_outage_and_successor_recovers_later() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.provider.now.store(370, Ordering::SeqCst);
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(370).await.is_err());
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    h.provider.lose_evidence.store(true, Ordering::SeqCst);
    let mut before = h.saved().await;
    assert!(h.adapter.emit_evidence(&mut before, 401).await.is_err());
    let before = h.saved().await;
    let frozen = before.windows[0].evidence.clone().unwrap();
    *h.state.sandbox.lock().unwrap() = None;
    h.control.deadlines.lock().unwrap().clear();
    h.control.pauses.lock().unwrap().clear();
    h.provider.fail_recover.store(true, Ordering::SeqCst);
    assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 0);
    let saved = h.saved().await;
    assert_eq!(saved.pending, before.pending);
    assert_eq!(saved.current, before.current);
    assert!(saved.windows.is_empty());
    assert_eq!(
        h.provider.evidence.lock().unwrap().as_slice(),
        &[frozen.clone(), frozen]
    );
    h.provider.fail_recover.store(false, Ordering::SeqCst);
    h.adapter.renew_due(at(561)).await.unwrap();
    let saved = h.saved().await;
    assert!(saved.pending.is_none());
    assert_eq!(saved.windows.len(), 1);
    assert_eq!(saved.windows[0].window.covered_from, 400);
    assert_eq!(saved.evidence_cursor, 1);
    assert_eq!(h.provider.requests.lock().unwrap().len(), 2);
    no_controls(&h);
}

#[tokio::test]
async fn stale_missing_sweep_cannot_lookup_or_overwrite_the_winner() {
    let h = pending().await;
    let before = h.saved().await;
    let (a, b) = tokio::join!(
        h.adapter.reconcile(before.clone(), at(500)),
        h.adapter.reconcile(before, at(500))
    );
    assert!(matches!(
        (a, b),
        (Ok(()), Err(FundingError::Conflict)) | (Err(FundingError::Conflict), Ok(()))
    ));
    assert_eq!(h.saved().await.windows.len(), 1);
    assert_eq!(h.provider.recoveries.lock().unwrap().len(), 2);
    no_controls(&h);
}

#[tokio::test]
async fn failed_adoption_does_not_replace_the_callers_saved_state() {
    for committed in [false, true] {
        let h = pending().await;
        let original = h.saved().await;
        let request = original.pending.clone().unwrap();
        let receipt = h.provider.receipts.lock().unwrap()[&request.request_id].clone();
        if committed {
            h.state.lose_adoption_ack.store(true, Ordering::SeqCst);
        } else {
            h.state.fail_adoption.store(true, Ordering::SeqCst);
        }
        let mut caller = original.clone();
        assert!(h
            .adapter
            .adopt_pending(&mut caller, &request, receipt)
            .await
            .is_err());
        assert_eq!(caller, original);
        assert_eq!(h.saved().await.pending.is_none(), committed);
    }
}

#[tokio::test]
async fn missing_recovery_preserves_a_valid_previous_generation_receipt_chain() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    let mut old = h.saved().await;
    h.adapter.emit_evidence(&mut old, 401).await.unwrap();
    let predecessor = old.current.unwrap();
    let mut replacement = h.sandbox.clone();
    replacement.runtime.as_mut().unwrap().provision_generation = Some(2);
    replacement.lifecycle.runtime_generation_high_water = 2;
    h.provider.now.store(401, Ordering::SeqCst);
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h
        .adapter
        .ensure_coverage(&replacement, at(401))
        .await
        .is_err());
    let original = h.saved().await;
    assert_eq!(
        original
            .pending
            .as_ref()
            .unwrap()
            .previous_window_id
            .as_ref(),
        Some(&predecessor.window_id)
    );
    *h.state.sandbox.lock().unwrap() = None;
    h.control.deadlines.lock().unwrap().clear();
    h.control.pauses.lock().unwrap().clear();
    assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 1);
    let saved = h.saved().await;
    assert_eq!(saved.runtime_generation, 2);
    assert_eq!(saved.windows.len(), 1);
    assert_eq!(saved.windows[0].window.runtime_generation, 2);
    assert_eq!(saved.coverage_start, 401);
    assert_eq!(h.provider.requests.lock().unwrap().len(), 2);
    assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 2);
    no_controls(&h);
}
