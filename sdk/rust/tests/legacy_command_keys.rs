#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{Error, agui::RunRequest, responses::CreateResponseRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

fn expected(path: &str, body: Value, key: &str, frames: String) -> Expected {
    let mut headers = wire::headers(true, "text/event-stream");
    headers.insert("content-type".into(), "application/json".into());
    headers.insert("idempotency-key".into(), key.into());
    Expected {
        method: "POST".into(),
        path: path.into(),
        body: Some(body),
        headers,
        status: 200,
        response_headers: BTreeMap::from([("content-type".into(), "text/event-stream".into())]),
        chunks: Some(vec![frames.into_bytes()]),
        ..Default::default()
    }
}

fn run() -> RunRequest {
    serde_json::from_value(json!({"threadId":"agt_one","runId":"correlation",
        "messages":[{"role":"user","content":"Hello"}]}))
    .unwrap()
}

#[tokio::test]
async fn agui_suggestion_retains_its_answer_condition_and_exact_text() {
    let server = WireServer::new().await;
    let body = json!({"threadId":"agt_one", "messages":[{"role":"user","content":"Add criteria.\nKeep limits."}],
        "forwardedProps":{"answerContext":{"turn_sequence":2,"answer_sequence":4}}});
    let request: RunRequest = serde_json::from_value(body.clone()).unwrap();
    server.expect(expected(
        "/api/v1/agui/run",
        body,
        "suggestion-key",
        "id: 6\ndata: {\"type\":\"RUN_STARTED\",\"threadId\":\"agt_one\",\"runId\":\"6\"}\n\n"
            .into(),
    ));
    let mut stream = server
        .client
        .agui()
        .run_with_key(&request, "suggestion-key")
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().run_id, "6");
    server.assert_drained();
}

#[tokio::test]
async fn agui_key_is_an_explicit_header_and_never_replaces_the_turn_or_correlation() {
    let server = WireServer::new().await;
    let request = run();
    server.expect(expected("/api/v1/agui/run", serde_json::to_value(&request).unwrap(), "retained-key",
        "id: 4\ndata: {\"type\":\"RUN_STARTED\",\"threadId\":\"agt_one\",\"runId\":\"4\"}\n\nid: 7\ndata: {\"type\":\"RUN_FINISHED\",\"threadId\":\"agt_one\",\"runId\":\"4\"}\n\n".into()));
    let mut stream = server
        .client
        .agui()
        .run_with_key(&request, "retained-key")
        .await
        .unwrap();
    assert_eq!(stream.next().await.unwrap().unwrap().run_id, "4");
    assert_eq!(stream.next().await.unwrap().unwrap().cursor, Some(7));
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn response_key_keeps_linear_request_and_connection_sequence_and_never_retries_eof() {
    let server = WireServer::new().await;
    let mut request = CreateResponseRequest::new("agt_one", "Hello");
    request.previous_response_id = Some("resp_agt_one_4".into());
    server.expect(expected("/api/v1/responses", serde_json::to_value(&request).unwrap(), "retained-key",
        "event: response.created\ndata: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"resp_agt_one_9\"}}\n\n".into()));
    let mut stream = server
        .client
        .responses()
        .create_with_key(&request, "retained-key")
        .await
        .unwrap();
    let event = stream.next().await.unwrap().unwrap();
    assert_eq!(event.sequence_number, 0);
    assert_eq!(event.payload["response"]["id"], "resp_agt_one_9");
    assert!(matches!(
        stream.next().await.unwrap(),
        Err(Error::Transport { .. })
    ));
    assert!(stream.next().await.is_none());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn invalid_keys_are_refused_before_either_legacy_post() {
    let server = WireServer::new().await;
    for key in [
        "".to_owned(),
        "x".repeat(257),
        "line\nbreak".into(),
        "\u{0085}".into(),
    ] {
        assert!(matches!(
            server.client.agui().run_with_key(&run(), &key).await,
            Err(Error::Validation { .. })
        ));
        assert!(matches!(
            server
                .client
                .responses()
                .create_with_key(&CreateResponseRequest::new("agt_one", "Hello"), &key)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}
