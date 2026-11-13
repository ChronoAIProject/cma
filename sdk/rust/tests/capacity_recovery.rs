#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    agents::{CapacityRecoveryOutcome, CapacityRecoveryRequest},
};
use serde_json::{Value, json};
use wire::{WireServer, json as response};

fn original() -> CapacityRecoveryRequest {
    CapacityRecoveryRequest {
        agent_id: "agt_recovery".into(),
        creation_key: "creation".into(),
        creation_fingerprint: "a".repeat(64),
        failure_seq: 10001,
        requested_at: "2026-11-11T00:00:00Z".into(),
    }
}
async fn command(server: &WireServer, method: &str) -> cma_sdk::Result<CapacityRecoveryOutcome> {
    let agents = server.client.agents();
    if method == "admit" {
        agents.admit_capacity_recovery(&original()).await
    } else {
        agents.resume_capacity_recovery(&original()).await
    }
}
fn expect(server: &WireServer, method: &str, status: u16, body: Value) {
    let mut expected = response(
        "POST",
        &format!("/api/v1/agents/agt_recovery/capacity-recovery/{method}"),
        Some(serde_json::to_value(original()).unwrap()),
        status,
        body,
        true,
    );
    expected
        .response_headers
        .insert("x-request-id".into(), "recovery-proof".into());
    expected
        .response_headers
        .insert("retry-after".into(), "2".into());
    server.expect(expected);
}
#[tokio::test]
async fn known_abort_is_typed_and_never_implicitly_retried() {
    for method in ["admit", "resume"] {
        let server = WireServer::new().await;
        expect(&server, method, 409, json!({"outcome": "known_abort"}));
        assert_eq!(
            command(&server, method).await.unwrap(),
            CapacityRecoveryOutcome::KnownAbort
        );
        server.assert_drained();
        assert_eq!(server.request_count(), 1);
        let _ = (&server.anonymous, &server.base_url);
    }
}
#[tokio::test]
async fn conflict_problem_keeps_api_error_and_correlation_metadata() {
    for method in ["admit", "resume"] {
        let server = WireServer::new().await;
        expect(
            &server,
            method,
            409,
            json!({"status":409,"code":"capacity_recovery_conflict","detail":"Recovery evidence changed."}),
        );
        let error = command(&server, method).await.unwrap_err();
        assert!(
            matches!(error, Error::Api { status: 409, code: Some(ref code), request_id: Some(ref request_id), retry_after: Some(delay), .. }
            if code == "capacity_recovery_conflict" && request_id == "recovery-proof" && delay.as_secs() == 2)
        );
        server.assert_drained();
        assert_eq!(server.request_count(), 1);
    }
}
#[tokio::test]
async fn malformed_and_status_mismatched_outcomes_refuse_without_retry() {
    for method in ["admit", "resume"] {
        for (status, body) in [
            (409, Value::Null),
            (409, json!({})),
            (409, json!({"outcome":"unknown"})),
            (409, json!({"outcome":"known_abort","unexpected":true})),
            (200, json!({"outcome":"known_abort"})),
            (202, json!({"outcome":"known_abort"})),
            (200, json!({"outcome":"unknown"})),
        ] {
            let server = WireServer::new().await;
            expect(&server, method, status, body);
            assert!(matches!(
                command(&server, method).await,
                Err(Error::Protocol { .. })
            ));
            server.assert_drained();
            assert_eq!(server.request_count(), 1);
        }
    }
}
