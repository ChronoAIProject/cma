use super::*;
use axum::http::Method;
use serde_json::json;

#[tokio::test]
async fn missing_sandbox_http_recovers_original_receipt_then_sends_only_retained_evidence() {
    let h = missing_tests::pending().await;
    let original = h.saved().await;
    let request = original.pending.unwrap();
    let window = h.provider.receipts.lock().unwrap()[&request.request_id].clone();
    let wire = serde_json::to_value(&window).unwrap();
    let server = http::Server::new(vec![
        http::reply(200, wire.clone()),
        http::reply(
            200,
            json!({
                "evidence_accepted": true, "window_id": window.window_id,
                "owner_id": window.owner_id, "sandbox_id": window.sandbox_id,
                "runtime_generation": window.runtime_generation
            }),
        ),
    ])
    .await;
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), h.state.clone(), h.control.clone());
    assert_eq!(adapter.renew_due(at(500)).await.unwrap(), 1);
    let saved = h.saved().await;
    assert!(saved.pending.is_none());
    assert_eq!(saved.windows[0].window.receipt, wire);
    assert_eq!(adapter.renew_due(at(561)).await.unwrap(), 1);
    assert!(h.saved().await.windows.is_empty());
    let calls = server.state.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, Method::GET);
    assert_eq!(
        calls[0].1,
        format!("/api/v1/billing/sandboxes/receipts/{}", request.request_id)
    );
    assert_eq!(calls[1].0, Method::POST);
    assert_eq!(
        calls[1].1,
        format!(
            "/api/v1/billing/sandboxes/windows/{}/evidence",
            window.window_id
        )
    );
    assert_eq!(calls[1].3["owner_id"], window.owner_id);
    assert_eq!(calls[1].3["runtime_generation"], window.runtime_generation);
    missing_tests::no_controls(&h);
}

#[tokio::test]
async fn missing_sandbox_http_404_outage_and_rate_limit_never_replay_reservation() {
    for status in [404, 500, 429] {
        let h = missing_tests::pending().await;
        let original = h.saved().await;
        let mut response = http::reply(status, json!({"error":"unavailable"}));
        response.retry_after = Some("120".into());
        let server = http::Server::new(vec![response]).await;
        let adapter = ProviderFundingAdapter::new(
            server.provider.clone(),
            h.state.clone(),
            h.control.clone(),
        );
        assert_eq!(adapter.renew_due(at(500)).await.unwrap(), 0);
        let saved = h.saved().await;
        assert_eq!(saved.pending, original.pending);
        assert_eq!(saved.current, original.current);
        assert!(saved.windows.is_empty());
        let expected_retry = if status == 429 { 620 } else { 560 };
        assert_eq!(saved.retry_at, expected_retry);
        assert_eq!(adapter.renew_due(at(expected_retry - 1)).await.unwrap(), 0);
        let calls = server.state.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Method::GET);
        assert!(calls[0].1.ends_with(&original.pending.unwrap().request_id));
        missing_tests::no_controls(&h);
    }
}

#[tokio::test]
async fn missing_sandbox_evidence_rate_limit_preserves_frozen_payload_and_retry_after() {
    let h = missing_tests::pending().await;
    let request = h.saved().await.pending.unwrap();
    let window = h.provider.receipts.lock().unwrap()[&request.request_id].clone();
    let mut limited = http::reply(429, json!({"error":"unavailable"}));
    limited.retry_after = Some("120".into());
    let server = http::Server::new(vec![
        http::reply(200, serde_json::to_value(&window).unwrap()),
        limited,
        http::reply(
            200,
            json!({
                "evidence_accepted": true, "window_id": window.window_id,
                "owner_id": window.owner_id, "sandbox_id": window.sandbox_id,
                "runtime_generation": window.runtime_generation
            }),
        ),
    ])
    .await;
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), h.state.clone(), h.control.clone());
    adapter.renew_due(at(500)).await.unwrap();
    assert_eq!(adapter.renew_due(at(561)).await.unwrap(), 0);
    let frozen = h.saved().await.windows[0].evidence.clone().unwrap();
    assert_eq!(h.saved().await.retry_at, 681);
    assert_eq!(adapter.renew_due(at(680)).await.unwrap(), 0);
    assert_eq!(server.state.calls.lock().unwrap().len(), 2);
    assert_eq!(adapter.renew_due(at(681)).await.unwrap(), 1);
    assert!(h.saved().await.windows.is_empty());
    let calls = server.state.calls.lock().unwrap();
    assert_eq!(calls[1].3, calls[2].3);
    assert_eq!(
        calls[1].3["intervals"].as_array().unwrap().len(),
        frozen.intervals.len()
    );
    missing_tests::no_controls(&h);
}
