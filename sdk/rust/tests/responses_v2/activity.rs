use super::{fixture, wire};
use cma_sdk::{
    Error,
    responses_v2::{CreateResponseRequest, ResponseEvent},
};
use serde_json::{Value, json};
use wire::WireServer;

fn activity_events(status: &str) -> Vec<Value> {
    let base = fixture::events();
    let activity = json!({"type":"mcp_call","id":"mcp_call_1","server_label":"cma","name":"Check proof","arguments":"","output":null,"error":null,"status":"in_progress"});
    let mut output = activity.clone();
    output["status"] = status.into();
    let mut result = base[..2].to_vec();
    result.push(json!({"type":"response.output_item.added","output_index":0,"item":activity}));
    result.push(
        json!({"type":"response.mcp_call.in_progress","output_index":0,"item_id":"mcp_call_1"}),
    );
    for mut event in base[2..8].iter().cloned() {
        event["output_index"] = 1.into();
        result.push(event);
    }
    result.push(json!({"type":format!("response.mcp_call.{status}"),"output_index":0,"item_id":"mcp_call_1"}));
    result.push(json!({"type":"response.output_item.done","output_index":0,"item":output}));
    let mut terminal = base[8].clone();
    terminal["response"]["output"] = json!([output, base[7]["item"]]);
    result.push(terminal);
    for (seq, event) in result.iter_mut().enumerate() {
        event["sequence_number"] = seq.into();
    }
    result
}

#[tokio::test]
async fn recorded_output_remains_typed_without_changing_standard_activity_fields() {
    use cma_sdk::responses_v2::{OutputItem, ResponseEventPayload};
    let server = WireServer::new().await;
    let mut events = activity_events("completed");
    let output = json!({"text":"native-computer-proof\n","truncated":true});
    events[11]["item"]["cma:output"] = output.clone();
    events[12]["response"]["output"][0]["cma:output"] = output;
    let request = CreateResponseRequest::new("agt_fixture", "Hello");
    server.expect(fixture::expected(
        "/api/v2/responses",
        Some(serde_json::to_value(&request).unwrap()),
        fixture::wire(&events),
    ));
    let mut stream = server
        .client
        .responses_v2()
        .create(&request, None)
        .await
        .unwrap();
    let mut observed = false;
    while let Some(event) = stream.next().await {
        if let ResponseEventPayload::Completed { response } = event.unwrap().payload {
            let OutputItem::Activity(activity) = &response.output[0] else {
                panic!("recorded activity missing")
            };
            let recorded = activity.recorded_output.as_ref().unwrap();
            assert_eq!(recorded.text, "native-computer-proof\n");
            assert!(recorded.truncated);
            assert!(activity.output.is_none());
            observed = true;
        }
    }
    assert!(observed);
    server.assert_drained();
}
#[tokio::test]
async fn mixed_text_activity_and_every_replay_cursor_accept_success_and_failure() {
    let server = WireServer::new().await;
    for status in ["completed", "failed"] {
        let events = activity_events(status);
        let request = CreateResponseRequest::new("agt_fixture", "Hello");
        server.expect(fixture::expected(
            "/api/v2/responses",
            Some(serde_json::to_value(&request).unwrap()),
            fixture::wire(&events),
        ));
        let mut stream = server
            .client
            .responses_v2()
            .create(&request, None)
            .await
            .unwrap();
        for event in &events {
            assert_eq!(
                stream.next().await.unwrap().unwrap(),
                serde_json::from_value::<ResponseEvent>(event.clone()).unwrap()
            );
        }
        assert!(stream.next().await.is_none());
        for cursor in 0..events.len() {
            let tail = &events[cursor + 1..];
            server.expect(fixture::expected(
                &format!("/api/v2/responses/resp_contract/events?starting_after={cursor}"),
                None,
                fixture::wire(tail),
            ));
            let mut stream = server
                .client
                .responses_v2()
                .replay("resp_contract", Some(cursor as u64))
                .await
                .unwrap();
            for event in tail {
                assert_eq!(
                    stream.next().await.unwrap().unwrap(),
                    serde_json::from_value::<ResponseEvent>(event.clone()).unwrap()
                );
            }
            assert!(stream.next().await.is_none());
        }
    }
    server.assert_drained();
}
#[tokio::test]
async fn malformed_activity_is_a_protocol_error() {
    let server = WireServer::new().await;
    for (index, field, value) in [
        (3, "item_id", json!("foreign")),
        (3, "type", json!("response.mcp_call.completed")),
        (10, "type", json!("response.mcp_call.in_progress")),
        (
            11,
            "item",
            json!({"type":"message","id":"mcp_call_1","role":"assistant","status":"completed","content":[]}),
        ),
        (
            11,
            "item",
            json!({"type":"mcp_call","id":"mcp_call_1","server_label":"cma","name":"Changed","arguments":"","output":null,"error":null,"status":"completed"}),
        ),
    ] {
        let mut events = activity_events("completed");
        events[index][field] = value;
        server.expect(fixture::expected(
            "/api/v2/responses/resp_contract/events",
            None,
            fixture::wire(&events),
        ));
        let mut stream = server
            .client
            .responses_v2()
            .replay("resp_contract", None)
            .await
            .unwrap();
        let mut failed = false;
        while let Some(event) = stream.next().await {
            if let Err(error) = event {
                assert!(matches!(error, Error::Protocol { .. }));
                failed = true;
                break;
            }
        }
        assert!(failed);
    }
    server.assert_drained();
}
