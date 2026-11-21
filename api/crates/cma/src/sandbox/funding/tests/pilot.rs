//! Cohort exclusions must never enter prepaid admission or reconciliation.
use super::*;

pub(crate) fn excluded_funding() -> Arc<dyn SandboxFundingAdapter> {
    let h = Harness::new();
    *h.provider.fail_quote.lock().unwrap() = Some(FundingError::NotEnrolled);
    h.adapter
}

#[tokio::test]
async fn non_cohort_generation_remains_unbilled_across_retry_restart_and_sweep() {
    let mut h = Harness::new();
    *h.provider.fail_quote.lock().unwrap() = Some(FundingError::NotEnrolled);
    h.sandbox.runtime = None;
    assert_eq!(h.fund(100).await.unwrap(), None);
    let state = h.saved().await;
    assert_eq!(state.unbilled_generation, Some(1));
    assert!(state.quote.is_none() && state.pending.is_none() && state.current.is_none());
    assert!(state.windows.is_empty() && state.last_error.is_none());
    assert_eq!(state.attempts, 0);

    // A later outage or flag enablement cannot enroll an already free runtime.
    *h.provider.fail_quote.lock().unwrap() = Some(transient());
    h.adapter = ProviderFundingAdapter::new(h.provider.clone(), h.state.clone(), h.control.clone());
    h.sandbox.runtime = sandbox().runtime;
    assert_eq!(h.fund(200).await.unwrap(), None);
    h.adapter.settle(&h.sandbox, at(201)).await.unwrap();
    assert_eq!(h.adapter.renew_due(at(500)).await.unwrap(), 0);
    h.adapter.reconcile(state, at(500)).await.unwrap();
    assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 1);
    assert!(h.provider.requests.lock().unwrap().is_empty());
    assert!(h.provider.receipts.lock().unwrap().is_empty());
    assert!(h.provider.evidence.lock().unwrap().is_empty());
    assert!(h.control.deadlines.lock().unwrap().is_empty());
    assert!(h.control.pauses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn non_cohort_resumes_are_unbilled_and_later_enrollment_is_prospective() {
    let mut h = Harness::new();
    *h.provider.fail_quote.lock().unwrap() = Some(FundingError::NotEnrolled);
    for generation in [1, 2, 4] {
        h.sandbox.lifecycle.runtime_generation_high_water = generation;
        assert_eq!(h.fund(100).await.unwrap(), None);
        assert_eq!(h.saved().await.unbilled_generation, Some(generation));
        h.adapter.settle(&h.sandbox, at(101)).await.unwrap();
    }
    assert!(h.provider.requests.lock().unwrap().is_empty());
    assert!(h.control.pauses.lock().unwrap().is_empty());
    *h.provider.fail_quote.lock().unwrap() = None;
    assert_eq!(h.fund(110).await.unwrap(), None);
    h.sandbox.lifecycle.runtime_generation_high_water = 5;
    h.provider.now.store(120, Ordering::SeqCst);
    assert_eq!(h.fund(120).await.unwrap(), Some(420));
    let state = h.saved().await;
    assert!(state.unbilled_generation.is_none());
    assert_eq!(state.coverage_start, 120);
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn billing_failures_are_not_cohort_exclusions() {
    for error in [
        transient(),
        FundingError::Invalid("producer authority rejected".into()),
        FundingError::Unfunded("insufficient credits".into()),
        FundingError::RateLimited(30),
    ] {
        let h = Harness::new();
        *h.provider.fail_quote.lock().unwrap() = Some(error.clone());
        assert_eq!(h.fund(100).await, Err(error));
        assert!(h.saved().await.unbilled_generation.is_none());
        assert!(h.provider.requests.lock().unwrap().is_empty());
        assert_eq!(h.control.pauses.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn stale_unfunded_quote_is_rechecked_before_attempting_a_hold() {
    let h = Harness::new();
    let mut state = SandboxBillingState::new(&h.sandbox, 1);
    h.state.save(&mut state).await.unwrap();
    state.quote = Some(
        h.provider
            .quote(QuoteRequest {
                owner_id: state.owner_id.clone(),
                sandbox_id: state.sandbox_id.clone(),
                allocation: maximum_allocation(&h.sandbox),
            })
            .await
            .unwrap(),
    );
    h.state.save(&mut state).await.unwrap();
    // The cohort/fleet closes after the quote and before durable reserve intent.
    *h.provider.fail_quote.lock().unwrap() = Some(FundingError::NotEnrolled);
    assert_eq!(h.fund(100).await.unwrap(), None);
    let saved = h.saved().await;
    assert_eq!(saved.unbilled_generation, Some(1));
    assert!(saved.quote.is_none() && saved.pending.is_none());
    assert!(h.provider.requests.lock().unwrap().is_empty());
    assert!(h.control.pauses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn admission_closure_never_discards_an_ambiguous_reservation() {
    let h = Harness::new();
    *h.provider.fail_reserve.lock().unwrap() = Some(transient());
    assert!(h.fund(100).await.is_err());
    let pending = h.saved().await.pending.unwrap();
    *h.provider.fail_reserve.lock().unwrap() = Some(FundingError::NotEnrolled);
    assert_eq!(h.fund(110).await, Err(FundingError::NotEnrolled));
    let state = h.saved().await;
    assert_eq!(state.pending.as_ref(), Some(&pending));
    assert!(state.unbilled_generation.is_none());
    *h.provider.fail_reserve.lock().unwrap() = None;
    assert_eq!(h.fund(130).await.unwrap(), Some(400));
    assert_eq!(h.provider.receipts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cohort_removal_after_paid_history_never_silently_bypasses_funding() {
    let mut h = Harness::new();
    h.fund(100).await.unwrap();
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    let mut state = h.saved().await;
    h.adapter.emit_evidence(&mut state, 401).await.unwrap();
    h.sandbox.lifecycle.runtime_generation_high_water = 2;
    *h.provider.fail_quote.lock().unwrap() = Some(FundingError::NotEnrolled);
    assert_eq!(h.fund(401).await, Err(FundingError::NotEnrolled));
    assert!(h.saved().await.unbilled_generation.is_none());
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert_eq!(h.provider.evidence.lock().unwrap().len(), 1);
}
