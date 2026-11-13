#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{
    Error,
    responses_v2::{ApprovalDecision, CommandState, ControlCommand, ControlRequest},
};
use serde_json::{Value, json};
use wire::WireServer;
const AGENT: &str = "agt_fixture";
const RESPONSE: &str = "resp_control_source";
const ROUTE: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/controls";

fn fixture(id: &str) -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/response-controls-v2.json"
    ))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == format!("response-controls-v2-{id}"))
        .unwrap()["response"]["body"]
        .clone()
}
fn intent() -> ControlRequest {
    ControlRequest {
        turn_sequence: 4,
        expected_agent_revision: 4,
        command: ControlCommand::Cancel {},
    }
}
fn expected(body: &ControlRequest, value: Value, status: u16, key: &str) -> wire::Expected {
    let mut expected = wire::json(
        "POST",
        ROUTE,
        Some(serde_json::to_value(body).unwrap()),
        status,
        value,
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), key.into());
    expected
}

#[tokio::test]
async fn observes_active_unsupported_and_queued_targets_through_exact_agent_response_routes() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    for name in ["owner-observe", "unsupported-observe", "queued-observe"] {
        let value = fixture(name);
        server.expect(wire::json("GET", ROUTE, None, 200, value.clone(), true));
        let observation = server
            .client
            .responses_v2()
            .observe_controls(AGENT, RESPONSE)
            .await
            .unwrap();
        assert_eq!(observation, serde_json::from_value(value).unwrap());
    }
    server.assert_drained();
}

#[tokio::test]
async fn submits_all_commands_without_changing_whitespace_observation_or_key() {
    let server = WireServer::new().await;
    let commands = [
        ControlCommand::Cancel {},
        ControlCommand::Steer {
            text: "  Review\nfirst  ".into(),
        },
        ControlCommand::ResolveApproval {
            approval_id: "approval-contract".into(),
            decision: ApprovalDecision::Allow,
        },
        ControlCommand::ResolveApproval {
            approval_id: "approval-contract".into(),
            decision: ApprovalDecision::Deny,
        },
        ControlCommand::ResolveApproval {
            approval_id: "approval-contract".into(),
            decision: ApprovalDecision::Abort,
        },
    ];
    for command in commands {
        let request = ControlRequest {
            command,
            ..intent()
        };
        server.expect(expected(&request, fixture("cancel"), 202, "retained key"));
        let value = server
            .client
            .responses_v2()
            .submit_control(AGENT, RESPONSE, &request, "retained key")
            .await
            .unwrap();
        assert_eq!(value.expected_agent_revision, 4);
        assert_eq!(value.target.turn_sequence, 4);
    }
    server.assert_drained();
}

#[tokio::test]
async fn recovery_reuses_original_intent_and_delivery_is_distinct_from_response_outcome() {
    let server = WireServer::new().await;
    for _ in 0..2 {
        server.expect(expected(
            &intent(),
            fixture("cancel-retry"),
            202,
            "same-key",
        ));
    }
    let first = server
        .client
        .responses_v2()
        .submit_control(AGENT, RESPONSE, &intent(), "same-key")
        .await
        .unwrap();
    let retry = server
        .client
        .responses_v2()
        .submit_control(AGENT, RESPONSE, &intent(), "same-key")
        .await
        .unwrap();
    assert_eq!(first, retry);
    let mut completed = fixture("cancel")["control"].clone();
    completed["status"] = json!("completed");
    completed["turn_sequence"] = json!(4);
    completed["delivery"] = json!("write_confirmed");
    server.expect(wire::json(
        "GET",
        "/api/v2/commands/cmd_cancel_contract",
        None,
        200,
        completed,
        true,
    ));
    server.expect(wire::json(
        "GET",
        ROUTE,
        None,
        200,
        fixture("owner-observe"),
        true,
    ));
    let status = server
        .client
        .responses_v2()
        .read_command("cmd_cancel_contract")
        .await
        .unwrap();
    assert!(status.status.is_terminal());
    assert_eq!(
        server
            .client
            .responses_v2()
            .observe_controls(AGENT, RESPONSE)
            .await
            .unwrap()
            .response
            .status,
        CommandState::InProgress
    );
    server.assert_drained();
}

#[tokio::test]
async fn preserves_delivery_uncertainty_and_typed_errors_without_automatic_retries() {
    let server = WireServer::new().await;
    for delivery in [
        "dispatch_fenced",
        "write_confirmed",
        "execution_observed",
        "pre_write_failed",
        "recovery_required",
        "runtime_lost",
    ] {
        let mut value = fixture("cancel")["control"].clone();
        value["status"] = json!("in_progress");
        value["turn_sequence"] = json!(4);
        value["delivery"] = json!(delivery);
        server.expect(wire::json(
            "GET",
            "/api/v2/commands/cmd_cancel_contract",
            None,
            200,
            value,
            true,
        ));
        let status = server
            .client
            .responses_v2()
            .read_command("cmd_cancel_contract")
            .await
            .unwrap();
        assert!(!status.status.is_terminal());
        assert_eq!(status.delivery.unwrap().as_str(), delivery);
    }
    for (name, status) in [
        ("cancel-conflict", 409),
        ("stale-target", 409),
        ("capability-refusal", 422),
        ("foreign-submit", 404),
        ("anonymous-submit", 401),
    ] {
        let value = fixture(name);
        let code = value["error"]["code"].as_str().unwrap().to_owned();
        server.expect(expected(&intent(), value, status, "key"));
        let before = server.request_count();
        let error = server
            .client
            .responses_v2()
            .submit_control(AGENT, RESPONSE, &intent(), "key")
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Api { code: Some(actual), .. } if actual == code));
        assert_eq!(server.request_count(), before + 1);
    }
    server.assert_drained();
}

#[tokio::test]
async fn strict_local_validation_refuses_injected_fields_invalid_keys_and_stale_numeric_shapes() {
    let server = WireServer::new().await;
    for key in ["", " ", " key", "key ", "bad\n", &"k".repeat(257)] {
        assert!(matches!(
            server
                .client
                .responses_v2()
                .submit_control(AGENT, RESPONSE, &intent(), key)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for request in [
        ControlRequest {
            turn_sequence: 0,
            ..intent()
        },
        ControlRequest {
            expected_agent_revision: u64::MAX,
            ..intent()
        },
        ControlRequest {
            command: ControlCommand::Steer { text: "  ".into() },
            ..intent()
        },
        ControlRequest {
            command: ControlCommand::Steer {
                text: "界".repeat(90_000),
            },
            ..intent()
        },
        ControlRequest {
            command: ControlCommand::ResolveApproval {
                approval_id: "bad\n".into(),
                decision: ApprovalDecision::Allow,
            },
            ..intent()
        },
    ] {
        assert!(matches!(
            server
                .client
                .responses_v2()
                .submit_control(AGENT, RESPONSE, &request, "key")
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for body in [
        r#"{"turn_sequence":4,"expected_agent_revision":4,"command":{"type":"cancel"},"actor":"forged"}"#,
        r#"{"turn_sequence":4,"turn_sequence":5,"expected_agent_revision":4,"command":{"type":"cancel"}}"#,
        r#"{"turn_sequence":4,"expected_agent_revision":4,"command":{"type":"cancel","text":"forged"}}"#,
    ] {
        assert!(serde_json::from_str::<ControlRequest>(body).is_err());
    }
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn retains_maximum_header_and_non_ascii_text_bytes() {
    let server = WireServer::new().await;
    let request = ControlRequest {
        command: ControlCommand::Steer {
            text: "界".repeat(80_000),
        },
        ..intent()
    };
    let key = "k".repeat(256);
    server.expect(expected(&request, fixture("cancel"), 202, &key));
    server
        .client
        .responses_v2()
        .submit_control(AGENT, RESPONSE, &request, &key)
        .await
        .unwrap();
    server.assert_drained();
}

#[tokio::test]
async fn malformed_foreign_or_missing_nullable_output_is_never_accepted() {
    let server = WireServer::new().await;
    for (path, value) in [
        ("/object", json!("other")),
        ("/response/receipt/agent_id", json!("other")),
        ("/response/receipt/response_id", json!("other")),
        ("/target/turn_sequence", json!(9)),
        ("/agent_revision", json!(0)),
        ("/runtime_support/cancel", json!("yes")),
        ("/response/delivery", json!("unknown")),
        ("/response/status", json!("unknown")),
        ("/response/failure", json!("secret")),
        ("/target", Value::Null),
    ] {
        let mut body = fixture("owner-observe");
        *body.pointer_mut(path).unwrap() = value;
        server.expect(wire::json("GET", ROUTE, None, 200, body, true));
        assert!(matches!(
            server
                .client
                .responses_v2()
                .observe_controls(AGENT, RESPONSE)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let mut body = fixture("owner-observe");
    body["response"].as_object_mut().unwrap().remove("failure");
    server.expect(wire::json("GET", ROUTE, None, 200, body, true));
    assert!(matches!(
        server
            .client
            .responses_v2()
            .observe_controls(AGENT, RESPONSE)
            .await,
        Err(Error::Protocol { .. })
    ));
    for (path, value) in [
        ("/target/response_id", json!("other")),
        ("/target/turn_sequence", json!(8)),
        ("/expected_agent_revision", json!(8)),
        ("/control/receipt/agent_id", json!("other")),
        ("/control/receipt/response_id", json!(RESPONSE)),
        ("/control/turn_sequence", json!(8)),
    ] {
        let mut body = fixture("cancel");
        *body.pointer_mut(path).unwrap() = value;
        server.expect(expected(&intent(), body, 202, "key"));
        assert!(matches!(
            server
                .client
                .responses_v2()
                .submit_control(AGENT, RESPONSE, &intent(), "key")
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.expect(wire::json(
        "GET",
        "/api/v2/commands/cmd_cancel_contract",
        None,
        200,
        fixture("source-status"),
        true,
    ));
    assert!(matches!(
        server
            .client
            .responses_v2()
            .read_command("cmd_cancel_contract")
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[test]
fn preserves_typed_revival_failures_in_command_status() {
    for failure in ["credential_setup_required", "connections_required"] {
        let mut value = fixture("queued-observe")["response"].clone();
        value["status"] = json!("failed");
        value["failure"] = json!(failure);
        let status: cma_sdk::responses_v2::CommandStatus = serde_json::from_value(value).unwrap();
        assert_eq!(status.failure.unwrap().as_str(), failure);
    }
}
