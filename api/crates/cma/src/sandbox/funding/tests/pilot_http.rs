use super::{
    http::{reply, Server},
    *,
};
use serde_json::json;

fn exclusion(message: &str) -> serde_json::Value {
    json!({"error":"billing_provider_unavailable","error_code":11302,"message":format!("Billing provider unavailable: {message}")})
}

#[tokio::test]
async fn nyxid_cohort_and_fleet_refusals_allow_create_and_resume_without_any_hold_request() {
    for message in [
        "CMA paid admission is disabled",
        "Sandbox owner is outside the billing rollout",
    ] {
        let server = Server::new(vec![
            reply(402, exclusion(message)),
            reply(402, exclusion(message)),
        ])
        .await;
        let state = Arc::new(MemoryState::default());
        let control = Arc::new(Control::default());
        let adapter =
            ProviderFundingAdapter::new(server.provider.clone(), state.clone(), control.clone());
        let mut sandbox = sandbox();
        sandbox.runtime = None;
        assert_eq!(
            adapter.ensure_coverage(&sandbox, at(100)).await.unwrap(),
            None
        );
        sandbox.runtime = fixture::sandbox().runtime;
        assert_eq!(
            adapter.ensure_coverage(&sandbox, at(101)).await.unwrap(),
            None
        );
        adapter.settle(&sandbox, at(102)).await.unwrap();
        sandbox.lifecycle.runtime_generation_high_water = 2;
        assert_eq!(
            adapter.ensure_coverage(&sandbox, at(103)).await.unwrap(),
            None
        );
        assert_eq!(adapter.renew_due(at(500)).await.unwrap(), 0);
        let saved = state.load(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(saved.unbilled_generation, Some(2));
        assert!(saved.pending.is_none() && saved.current.is_none() && saved.windows.is_empty());
        assert!(control.pauses.lock().unwrap().is_empty());
        assert!(control.deadlines.lock().unwrap().is_empty());
        let calls = server.state.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls
            .iter()
            .all(|call| call.1 == "/api/v1/billing/sandboxes/quotes"));
    }
}

#[tokio::test]
async fn unknown_or_malformed_admission_refusals_cannot_enable_free_compute() {
    for (status, body) in [
        (402, exclusion("Billing provider is unavailable")),
        (
            402,
            json!({"error":"billing_provider_unavailable","message":"CMA paid admission is disabled"}),
        ),
        (
            402,
            json!({"error":"billing_provider_unavailable","error_code":11301,"message":"CMA paid admission is disabled"}),
        ),
        (403, exclusion("CMA paid admission is disabled")),
        (503, exclusion("CMA paid admission is disabled")),
        (
            402,
            json!({"error":"insufficient_credits","error_code":11300,"message":"CMA paid admission is disabled"}),
        ),
        (
            402,
            json!({"error":"billing_not_configured","error_code":11301,"message":"CMA paid admission is disabled"}),
        ),
    ] {
        let server = Server::new(vec![reply(status, body)]).await;
        let state = Arc::new(MemoryState::default());
        let control = Arc::new(Control::default());
        let adapter =
            ProviderFundingAdapter::new(server.provider.clone(), state.clone(), control.clone());
        let sandbox = sandbox();
        let error = adapter
            .ensure_coverage(&sandbox, at(100))
            .await
            .unwrap_err();
        assert_ne!(error, FundingError::NotEnrolled);
        assert!(state
            .load(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .unbilled_generation
            .is_none());
        assert_eq!(control.pauses.lock().unwrap().len(), 1);
        assert_eq!(server.state.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn exclusion_envelope_on_receipt_lookup_is_not_proof_of_no_hold() {
    let server = Server::new(vec![reply(
        402,
        exclusion("CMA paid admission is disabled"),
    )])
    .await;
    assert!(matches!(
        server.provider.recover("r").await,
        Err(FundingError::Retryable(_))
    ));
}
