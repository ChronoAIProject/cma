#[path = "queued_cancellation_v2/fixture.rs"]
mod fixture;
#[path = "queued_cancellation_v2/validation.rs"]
mod validation;
#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{
    Error,
    responses_v2::{CommandState, QueuedCancellationRequest},
};
use fixture::*;
use serde_json::{Value, json};
use wire::WireServer;

#[tokio::test]
async fn observes_queued_and_unbound_channel_waiting_sources_without_mutating() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    for state in ["queued", "waiting_action"] {
        let mut body = observation();
        body["response"]["status"] = json!(state);
        server.expect(wire::json("GET", ROUTE, None, 200, body.clone(), true));
        let actual = server
            .client
            .responses_v2()
            .observe_queued_cancellation(AGENT, RESPONSE)
            .await
            .unwrap();
        assert_eq!(actual, serde_json::from_value(body).unwrap());
    }
    server.assert_drained();
}

#[tokio::test]
async fn queued_cancellation_retains_original_target_and_current_source_view_on_replay() {
    let server = WireServer::new().await;
    let mut original = None;
    for state in ["cancelled", "completed"] {
        let mut body = receipt();
        body["response"]["status"] = json!(state);
        server.expect(expected(body.clone(), 200, "saved key"));
        let result = server
            .client
            .responses_v2()
            .cancel_queued_response(AGENT, RESPONSE, &intent(), "saved key")
            .await
            .unwrap();
        assert_eq!(result, serde_json::from_value(body).unwrap());
        assert_eq!(result.cancellation.status, CommandState::Completed);
        let stable = (result.cancellation, result.target);
        if let Some(first) = &original {
            assert_eq!(&stable, first);
        } else {
            original = Some(stable);
        }
    }
    server.assert_drained();
}

#[tokio::test]
async fn uncertain_reply_is_not_retried_and_explicit_recovery_preserves_key_and_body() {
    let server = WireServer::new().await;
    let mut lost = expected(receipt(), 200, "persisted-key");
    lost.response = b"{".to_vec();
    server.expect(lost);
    let result = server
        .client
        .responses_v2()
        .cancel_queued_response(AGENT, RESPONSE, &intent(), "persisted-key")
        .await;
    assert!(matches!(result, Err(Error::Protocol { .. })));
    assert_eq!(server.request_count(), 1);
    server.expect(expected(receipt(), 200, "persisted-key"));
    server
        .client
        .responses_v2()
        .cancel_queued_response(AGENT, RESPONSE, &intent(), "persisted-key")
        .await
        .unwrap();
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn preserves_typed_refusal_codes_without_automatic_mutation_retries() {
    let server = WireServer::new().await;
    for (status, code) in [
        (409, "queued_response_cancellation_conflict"),
        (409, "queued_response_cancellation_not_admitted"),
        (409, "agent_command_key_conflict"),
        (422, "invalid_queued_cancellation_request"),
        (404, "response_not_found"),
        (401, "authentication_required"),
        (503, "response_unavailable"),
    ] {
        let error = json!({"error":{"type":"invalid_request_error","code":code,"message":"Refused","param":null}});
        server.expect(expected(error, status, "saved-key"));
        let before = server.request_count();
        let result = server
            .client
            .responses_v2()
            .cancel_queued_response(AGENT, RESPONSE, &intent(), "saved-key")
            .await;
        assert!(matches!(result, Err(Error::Api { code: Some(actual), .. }) if actual == code));
        assert_eq!(server.request_count(), before + 1);
    }
    server.assert_drained();
}

#[tokio::test]
async fn key_and_source_validation_happens_before_http_and_accepts_maximum_header() {
    let server = WireServer::new().await;
    for key in ["", " ", " key", "key ", "bad\n", &"k".repeat(257)] {
        assert!(matches!(
            server
                .client
                .responses_v2()
                .cancel_queued_response(AGENT, RESPONSE, &intent(), key)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for body in [
        QueuedCancellationRequest {
            source_command_id: "".into(),
            ..intent()
        },
        QueuedCancellationRequest {
            source_command_id: "bad\n".into(),
            ..intent()
        },
        QueuedCancellationRequest {
            expected_command_revision: 0,
            ..intent()
        },
        QueuedCancellationRequest {
            expected_command_revision: u64::MAX,
            ..intent()
        },
    ] {
        assert!(matches!(
            server
                .client
                .responses_v2()
                .cancel_queued_response(AGENT, RESPONSE, &body, "saved")
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for body in [
        r#"{"source_command_id":"source","expected_command_revision":1,"actor":"forged"}"#,
        r#"{"source_command_id":"source","expected_command_revision":1,"expected_command_revision":2}"#,
        r#"{"source_command_id":"source","expected_agent_revision":1}"#,
    ] {
        assert!(serde_json::from_str::<QueuedCancellationRequest>(body).is_err());
    }
    assert_eq!(server.request_count(), 0);
    let key = "k".repeat(256);
    server.expect(expected(receipt(), 200, &key));
    server
        .client
        .responses_v2()
        .cancel_queued_response(AGENT, RESPONSE, &intent(), &key)
        .await
        .unwrap();
    server.assert_drained();
}
