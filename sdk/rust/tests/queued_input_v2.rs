#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{Error, responses_v2::QueuedInputRequest};
use serde_json::{Value, json};
use wire::WireServer;
const AGENT: &str = "agt_fixture";
const RESPONSE: &str = "resp_control_source";
const LIST: &str = "/api/v2/agents/agt_fixture/queued-inputs";
const EDIT: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/queued-input";
fn source() -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/response-controls-v2.json"
    ))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "response-controls-v2-queued-observe")
        .unwrap()["response"]["body"]["response"]
        .clone()
}
fn intent() -> QueuedInputRequest {
    QueuedInputRequest {
        source_command_id: "cmd_control_source".into(),
        expected_command_revision: 1,
        text: "revised text with attachment".into(),
    }
}
fn receipt() -> Value {
    let mut edit = source();
    edit["receipt"]["command_id"] = json!("cmd_edit");
    edit["receipt"]["response_id"] = Value::Null;
    edit["receipt"]["agent_order"] = json!(2);
    edit["status"] = json!("completed");
    json!({"object":"cma.response.queued_input", "edit":edit,"response":source(),"target":{"response_id":RESPONSE,"source_command_id":"cmd_control_source","expected_command_revision":1},"command_revision":2,"text":intent().text})
}
fn expected(value: Value) -> wire::Expected {
    let mut expected = wire::json(
        "POST",
        EDIT,
        Some(serde_json::to_value(intent()).unwrap()),
        200,
        value,
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "saved-edit".into());
    expected
}
#[tokio::test]
async fn queued_input_lists_and_recovers_exact_edit_without_automatic_retry() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    server.expect(wire::json("GET", LIST, None, 200, json!({"object":"cma.agent.queued_inputs", "agent_id":AGENT,"next_after":null,"data":[{"response":source(),"command_revision":1,"text":"original","editable":true}]}), true));
    assert_eq!(
        server
            .client
            .responses_v2()
            .list_queued_inputs(AGENT)
            .await
            .unwrap()
            .data
            .len(),
        1
    );
    let mut lost = expected(receipt());
    lost.response = b"{".to_vec();
    server.expect(lost);
    assert!(matches!(
        server
            .client
            .responses_v2()
            .edit_queued_input(AGENT, RESPONSE, &intent(), "saved-edit")
            .await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(server.request_count(), 2);
    server.expect(expected(receipt()));
    let recovered = server
        .client
        .responses_v2()
        .edit_queued_input(AGENT, RESPONSE, &intent(), "saved-edit")
        .await
        .unwrap();
    assert_eq!(recovered.text, intent().text);
    assert_eq!(recovered.command_revision, 2);
    server.assert_drained();
}
#[tokio::test]
async fn queued_input_rejects_bad_content_and_mismatched_receipt() {
    let server = WireServer::new().await;
    for text in [String::new(), "x".repeat(262145)] {
        let mut input = intent();
        input.text = text;
        assert!(matches!(
            server
                .client
                .responses_v2()
                .edit_queued_input(AGENT, RESPONSE, &input, "saved-edit")
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
    let mut mismatched = receipt();
    mismatched["text"] = json!("changed");
    server.expect(expected(mismatched));
    assert!(matches!(
        server
            .client
            .responses_v2()
            .edit_queued_input(AGENT, RESPONSE, &intent(), "saved-edit")
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn queued_input_loads_all_64_maximum_escaped_messages_in_bounded_pages() {
    let server = WireServer::new().await;
    let text = "\u{1}".repeat(256 * 1024);
    for start in (0..64).step_by(4) {
        let data: Vec<_> = (start + 1..=start + 4)
            .map(|order| {
                let mut response = source();
                response["receipt"]["command_id"] = json!(format!("cmd_{order}"));
                response["receipt"]["response_id"] = json!(format!("resp_{order}"));
                response["receipt"]["agent_order"] = json!(order);
                json!({"response":response,"command_revision":1,"text":text,"editable":true})
            })
            .collect();
        let body = json!({"object":"cma.agent.queued_inputs","agent_id":AGENT,"data":data,"next_after":if start + 4 < 64 { Some(start + 4) } else { None }});
        assert!(serde_json::to_vec(&body).unwrap().len() < 8 * 1024 * 1024);
        let path = if start == 0 {
            LIST.into()
        } else {
            format!("{LIST}?after={start}")
        };
        server.expect(wire::json("GET", &path, None, 200, body, true));
    }
    let queue = server
        .client
        .responses_v2()
        .list_queued_inputs(AGENT)
        .await
        .unwrap();
    assert_eq!(queue.data.len(), 64);
    assert!(queue.data.iter().all(|item| item.text == text));
    assert_eq!(server.request_count(), 16);
    server.assert_drained();
}
#[tokio::test]
async fn queued_input_refuses_forged_and_repeated_pagination_cursors() {
    let server = WireServer::new().await;
    let item = json!({"response":source(),"command_revision":1,"text":"message","editable":true});
    for next in [2, 0] {
        server.expect(wire::json("GET",LIST,None,200,json!({"object":"cma.agent.queued_inputs","agent_id":AGENT,"data":[item],"next_after":next}),true));
        assert!(matches!(
            server.client.responses_v2().list_queued_inputs(AGENT).await,
            Err(Error::Protocol { .. })
        ));
    }
    server.expect(wire::json(
        "GET",
        LIST,
        None,
        200,
        json!({"object":"cma.agent.queued_inputs","agent_id":AGENT,"data":[item],"next_after":1}),
        true,
    ));
    server.expect(wire::json("GET",&format!("{LIST}?after=1"),None,200,json!({"object":"cma.agent.queued_inputs","agent_id":AGENT,"data":[item],"next_after":null}),true));
    assert!(matches!(
        server.client.responses_v2().list_queued_inputs(AGENT).await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}
