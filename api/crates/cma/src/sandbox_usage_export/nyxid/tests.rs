use super::*;
use crate::producer_token::oauth::tests::{issued, Server};
use serde_json::json;

mod compatibility;
mod safety;

fn event() -> FrozenUsageEvent {
    let payload =
        include_bytes!("../../../tests/fixtures/cma_usage_v1/allocated.canonical.json").to_vec();
    let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    use sha2::{Digest, Sha256};
    FrozenUsageEvent {
        event_id: value["event_id"].as_str().unwrap().into(),
        stream_id: value["stream_id"].as_str().unwrap().into(),
        namespace: value["producer_namespace"].as_str().unwrap().into(),
        sequence: value["sequence"].as_str().unwrap().parse().unwrap(),
        produced_at_ms: value["produced_at_ms"].as_str().unwrap().parse().unwrap(),
        digest: format!("{:x}", Sha256::digest(&payload)),
        payload,
    }
}

fn receipt(event: &FrozenUsageEvent) -> String {
    json!({"receipt":{"receipt_id":uuid::Uuid::new_v4().to_string(),"event_id":event.event_id,
        "stream_id":event.stream_id,"sequence":event.sequence.to_string(),"payload_digest":event.digest,
        "accepted_at_ms":event.produced_at_ms.to_string(),"custody":"persisted"},
        "processing":{"validation_status":"waiting_for_predecessor","validation_reason":null,
            "owner_status":"owner_unresolved","financial_status":"nonbillable_shadow"}}).to_string()
}

fn cursor(event: &FrozenUsageEvent) -> serde_json::Value {
    json!({"stream_id":event.stream_id,"contiguous_custody_sequence":"0",
        "contiguous_validated_sequence":"0","highest_received_sequence":"2",
        "custody_chain_head":null,"validated_chain_head":null,
        "missing_ranges":[{"first":"1","last":"1"}],"next_page":null})
}

fn transport(server: &Server) -> NyxIdUsageTransport {
    NyxIdUsageTransport::with_tokens(
        server.url.clone(),
        ClientCredentialsProvider::new(
            &server.url,
            "fixture-client".into(),
            "fixture-secret".into(),
            ProducerScope::Usage,
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn token_rotation_and_transport_restart_keep_exact_frozen_bytes_and_usage_scopes() {
    let event = event();
    let server = Server::new(vec![
        issued("first", 3600),
        (401, String::new()),
        issued("second", 3600),
        (201, receipt(&event)),
        issued("third", 3600),
        (200, receipt(&event)),
    ])
    .await;
    transport(&server).submit(&event).await.unwrap();
    transport(&server).submit(&event).await.unwrap();
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    for index in [1, 3, 5] {
        assert_eq!(calls[index].0, "/api/v1/billing/sandboxes/usage-events");
        assert_eq!(calls[index].2.as_bytes(), event.payload);
        assert_eq!(calls[index].1.get_all("authorization").iter().count(), 1);
    }
    assert_eq!(calls[1].1["authorization"], "Bearer first");
    assert_eq!(calls[3].1["authorization"], "Bearer second");
    for index in [0, 2, 4] {
        let form: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(calls[index].2.as_bytes()).collect();
        assert_eq!(
            form["scope"],
            "billing:sandbox-usage:write billing:sandbox-usage:read"
        );
    }
}

#[tokio::test]
async fn response_identity_and_custody_are_required_for_acknowledgement() {
    let event = event();
    for field in [
        "event_id",
        "stream_id",
        "sequence",
        "payload_digest",
        "accepted_at_ms",
        "custody",
    ] {
        let mut wrong: serde_json::Value = serde_json::from_str(&receipt(&event)).unwrap();
        wrong["receipt"][field] = "wrong".into();
        let server = Server::new(vec![issued("test", 3600), (201, wrong.to_string())]).await;
        assert!(transport(&server).submit(&event).await.is_err(), "{field}");
    }
    for body in ["{}", r#"{"accepted":true}"#] {
        let server = Server::new(vec![issued("test", 3600), (200, body.into())]).await;
        assert!(transport(&server).submit(&event).await.is_err());
    }
}

#[tokio::test]
async fn funding_status_never_turns_evidence_into_a_success_or_an_enrollment_skip() {
    for (status, body, expected) in [
        (429, "", TransportError::RateLimited(30)),
        (503, "", TransportError::Unavailable),
        (403, "", TransportError::Forbidden),
        (402, "", TransportError::Invalid),
        (
            409,
            r#"{"error":"stream_not_open"}"#,
            TransportError::Prerequisite,
        ),
        (
            409,
            r#"{"error":"predecessor_missing"}"#,
            TransportError::Prerequisite,
        ),
        (
            409,
            r#"{"error":"digest_conflict"}"#,
            TransportError::Conflict,
        ),
        (422, "", TransportError::Invalid),
    ] {
        let server = Server::new(vec![issued("test", 3600), (status, body.into())]).await;
        assert_eq!(transport(&server).submit(&event()).await, Err(expected));
        assert_eq!(server.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn receipt_and_cursor_reads_use_independent_evidence_routes() {
    let event = event();
    let cursor = cursor(&event);
    let server = Server::new(vec![
        issued("test", 3600),
        (404, String::new()),
        (200, cursor.to_string()),
        (200, receipt(&event)),
    ])
    .await;
    let transport = transport(&server);
    assert!(transport.receipt(&event.event_id).await.unwrap().is_none());
    let cursor = transport.cursor(&event.stream_id).await.unwrap().unwrap();
    assert_eq!(cursor.validate(&event.stream_id).unwrap(), 0);
    transport
        .receipt(&event.event_id)
        .await
        .unwrap()
        .unwrap()
        .validate(&event)
        .unwrap();
    let calls = server.calls.lock().unwrap();
    assert_eq!(
        calls[1].0,
        format!(
            "/api/v1/billing/sandboxes/usage-receipts/{}",
            event.event_id
        )
    );
    assert_eq!(
        calls[2].0,
        format!(
            "/api/v1/billing/sandboxes/usage-cursors/{}",
            event.stream_id
        )
    );
    assert!(calls[1..].iter().all(|call| call.2.is_empty()));
}
