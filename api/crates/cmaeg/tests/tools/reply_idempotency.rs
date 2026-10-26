use super::support::{problem, App};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn optional_reply_header_preserves_body_and_operation_id() {
    let app = App::new().await;
    let document: Value = app
        .fixture
        .client
        .get(format!("{}/openapi.json", app.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reply = &document["paths"]["/events/{event_ref}/replies"]["post"];
    assert_eq!(reply["operationId"], "replyToEvent");
    assert_eq!(
        reply["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/OutboundMessage"
    );
    let header = reply["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "Idempotency-Key")
        .unwrap();
    assert_eq!(header["in"], "header");
    assert_ne!(header["required"], true);
    assert_eq!(header["schema"]["minLength"], 1);
    assert_eq!(header["schema"]["maxLength"], 128);
    assert_eq!(header["schema"]["pattern"], "^[!-~]+$");
}

#[tokio::test]
async fn malformed_or_duplicate_headers_are_rejected_before_any_delivery() {
    let app = App::new().await;
    for key in [
        "".to_owned(),
        "two words".into(),
        "\t".into(),
        "x".repeat(129),
    ] {
        problem(
            app.fixture
                .client
                .post(app.event_url("replies"))
                .bearer_auth(&app.key)
                .header("Idempotency-Key", key)
                .json(&json!({"text":"must not send"}))
                .send()
                .await
                .unwrap(),
            400,
            "invalid_request",
        )
        .await;
    }
    problem(
        app.fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .header("Idempotency-Key", "same")
            .header("Idempotency-Key", "same")
            .json(&json!({"text":"must not send"}))
            .send()
            .await
            .unwrap(),
        400,
        "invalid_request",
    )
    .await;
    let invalid = reqwest::header::HeaderValue::from_bytes(&[0x80]).unwrap();
    problem(
        app.fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .header("Idempotency-Key", invalid)
            .json(&json!({"text":"must not send"}))
            .send()
            .await
            .unwrap(),
        400,
        "invalid_request",
    )
    .await;
    assert!(app.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn malformed_mcp_keys_are_rejected_before_any_delivery() {
    let app = App::new().await;
    for key in [
        json!(""),
        json!("two words"),
        json!("\n"),
        json!("é"),
        json!("x".repeat(129)),
        json!(17),
    ] {
        let response: Value = app.rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"replyToEvent", "arguments":{
                "event_ref":app.event_ref.as_str(),"message":{"text":"must not send"},"idempotency_key":key
            }
        }})).await.json().await.unwrap();
        assert_eq!(response["error"]["data"]["code"], "invalid_request");
    }
    assert!(app.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_requests_keep_their_existing_non_idempotent_behavior_and_no_new_storage() {
    let app = App::new().await;
    assert!(app.state.routed_replies.is_none());
    // The legacy fixture only serves channel record GET. Any new durable-record
    // operation would fail; both old requests and accepted optional keys must
    // still reach the original delivery engine on every invocation.
    let mut expected = 0;
    let maximum_key = "x".repeat(128);
    for key in [
        None,
        None,
        Some("stable"),
        Some("stable"),
        Some("!"),
        Some(maximum_key.as_str()),
    ] {
        let mut request = app
            .fixture
            .client
            .post(app.event_url("replies"))
            .bearer_auth(&app.key)
            .json(&json!({"text":"identical legacy send"}));
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        let receipt: Value = response.json().await.unwrap();
        assert_eq!(receipt["delivery"]["outcome"], "confirmed");
        expected += 1;
        assert_eq!(app.requests.lock().unwrap().len(), expected);
    }
    let reads = app.fixture.backend.records.load(Ordering::SeqCst);
    for _ in 0..2 {
        let response: Value = app.rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"replyToEvent", "arguments":{
                "event_ref":app.event_ref.as_str(),"message":{"text":"identical legacy send"},"idempotency_key":"stable"
            }
        }})).await.json().await.unwrap();
        assert_eq!(
            response["result"]["structuredContent"]["delivery"]["outcome"],
            "confirmed"
        );
        expected += 1;
    }
    assert_eq!(app.requests.lock().unwrap().len(), expected);
    assert_eq!(app.fixture.backend.records.load(Ordering::SeqCst), reads);
}
