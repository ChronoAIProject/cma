#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{Error, responses_v2::ResponseInterruptionRequest};
use serde_json::{Value, json};
use wire::WireServer;
const AGENT: &str = "agt_fixture";
const RESPONSE: &str = "resp_control_source";
const ROUTE: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/interrupt";

fn intent() -> ResponseInterruptionRequest {
    ResponseInterruptionRequest {
        turn_sequence: 4,
        expected_agent_revision: 4,
        text: "  New task\nAttachment: /home/attachments/design.png  ".into(),
    }
}
fn receipt() -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/response-controls-v2.json"
    ))
    .unwrap();
    let original = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "response-controls-v2-cancel")
        .unwrap()["response"]["body"]
        .clone();
    let control = original["control"].clone();
    let mut response = control.clone();
    response["receipt"]["command_id"] = json!("cmd_replacement");
    response["receipt"]["response_id"] = json!("resp_replacement");
    response["receipt"]["agent_order"] =
        json!(control["receipt"]["agent_order"].as_u64().unwrap() + 1);
    json!({"object":"cma.response.interruption","control":control,"response":response,
        "target":original["target"],"expected_agent_revision":4})
}
fn expected(value: Value, status: u16) -> wire::Expected {
    let mut request = wire::json(
        "POST",
        ROUTE,
        Some(serde_json::to_value(intent()).unwrap()),
        status,
        value,
        true,
    );
    request
        .headers
        .insert("idempotency-key".into(), "same-interruption".into());
    request
}
#[tokio::test]
async fn interruption_preserves_exact_text_attachments_target_and_retry_identity() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    for _ in 0..2 {
        server.expect(expected(receipt(), 202));
    }
    let first = server
        .client
        .responses_v2()
        .interrupt_with_input(AGENT, RESPONSE, &intent(), "same-interruption")
        .await
        .unwrap();
    let replay = server
        .client
        .responses_v2()
        .interrupt_with_input(AGENT, RESPONSE, &intent(), "same-interruption")
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(
        first.response.receipt.response_id.as_deref(),
        Some("resp_replacement")
    );
    assert_eq!(first.target.response_id, RESPONSE);
    server.assert_drained();
}
#[tokio::test]
async fn interruption_rejects_mismatched_or_non_atomic_receipts() {
    let server = WireServer::new().await;
    for (path, value) in [
        ("/target/response_id", json!("resp_other")),
        ("/target/turn_sequence", json!(5)),
        ("/expected_agent_revision", json!(5)),
        ("/control/receipt/agent_id", json!("agt_other")),
        ("/response/receipt/agent_id", json!("agt_other")),
        ("/control/receipt/response_id", json!(RESPONSE)),
        ("/response/receipt/response_id", json!(RESPONSE)),
        ("/response/receipt/response_id", Value::Null),
        (
            "/response/receipt/command_id",
            receipt()["control"]["receipt"]["command_id"].clone(),
        ),
        ("/response/receipt/agent_order", json!(900)),
        ("/control/turn_sequence", json!(5)),
    ] {
        let mut response = receipt();
        *response.pointer_mut(path).unwrap() = value;
        server.expect(expected(response, 202));
        assert!(
            matches!(
                server
                    .client
                    .responses_v2()
                    .interrupt_with_input(AGENT, RESPONSE, &intent(), "same-interruption")
                    .await,
                Err(Error::Protocol { .. })
            ),
            "accepted corrupted {path}"
        );
    }
    server.assert_drained();
}
#[tokio::test]
async fn interruption_does_not_retry_definitive_refusal_or_submit_invalid_input() {
    let server = WireServer::new().await;
    server.expect(expected(json!({"error":{"type":"invalid_request_error","code":"response_control_not_admitted","message":"Target is no longer active.","param":null}}), 409));
    assert!(
        server
            .client
            .responses_v2()
            .interrupt_with_input(AGENT, RESPONSE, &intent(), "same-interruption")
            .await
            .is_err()
    );
    for request in [
        ResponseInterruptionRequest {
            text: "  ".into(),
            ..intent()
        },
        ResponseInterruptionRequest {
            turn_sequence: 0,
            ..intent()
        },
        ResponseInterruptionRequest {
            expected_agent_revision: 0,
            ..intent()
        },
    ] {
        assert!(
            server
                .client
                .responses_v2()
                .interrupt_with_input(AGENT, RESPONSE, &request, "same-interruption")
                .await
                .is_err()
        );
    }
    assert!(
        server
            .client
            .responses_v2()
            .interrupt_with_input(AGENT, RESPONSE, &intent(), " bad key ")
            .await
            .is_err()
    );
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}
