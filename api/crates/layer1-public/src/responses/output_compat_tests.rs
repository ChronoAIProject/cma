mod fixture;

use std::{sync::atomic::Ordering, time::Duration};

use axum::body::to_bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};

use fixture::*;

async fn frame(
    stream: &mut (impl Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
) -> Value {
    let bytes = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap()
}

fn events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn output_only_legacy_frames_preserve_recorded_activity_across_reads_and_replay() {
    let mut fixture = fixture().await;
    let response = fixture.create().await;
    let receipt = response.headers()["x-cma-command-id"].clone();
    let mut stream = response.into_body().into_data_stream();
    let opening = frame(&mut stream).await;
    let response_id = opening["response"]["id"].as_str().unwrap();
    assert_eq!(frame(&mut stream).await["type"], "response.in_progress");
    fixture.update(json!({"sessionUpdate":"tool_call", "toolCallId":"command-proof", "title":"printf response-compat-proof", "kind":"execute", "status":"in_progress"})).await;
    let started = frame(&mut stream).await;
    assert!(started["item"].get("output").is_none());
    fixture.update(json!({"sessionUpdate":"tool_call_update", "toolCallId":"command-proof", "content":[{"type":"content", "content":{"type":"text", "text":"response-compat-proof\n"}}]})).await;
    let reported = frame(&mut stream).await;
    assert_eq!(reported["response_id"], response_id);
    assert_eq!(reported["item"]["id"], started["item"]["id"]);
    assert_eq!(reported["item"]["call_id"], "command-proof");
    assert_eq!(reported["item"]["kind"], "execute");
    assert_eq!(reported["item"]["target"], "printf response-compat-proof");
    assert_eq!(reported["item"]["status"], "in_progress");
    assert_eq!(
        reported["item"]["output"],
        json!({"text":"response-compat-proof\n", "truncated":false})
    );
    assert_eq!(reported["sequence_number"], 3);
    let read_path = format!("/api/v1/responses/{response_id}");
    let before = fixture.read(&read_path).await;
    assert_eq!(before["status"], "in_progress");
    assert_eq!(before["output"], json!([]));
    let items_path = format!("/api/v1/conversations/{AGENT}/items");
    let items = fixture.read(&items_path).await;
    assert_eq!(items["data"][0], reported["item"]);
    drop(stream);
    fixture.update(json!({"sessionUpdate":"tool_call_update", "toolCallId":"command-proof", "status":"completed"})).await;
    fixture.update(json!({"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"Command observed"}})).await;
    fixture.update(json!({"end_turn":true})).await;
    let after = fixture.read(&read_path).await;
    assert_eq!(after["status"], "completed");
    assert_eq!(after["output"][0]["content"][0]["text"], "Command observed");
    let replay = fixture.create().await;
    assert_eq!(replay.headers()["x-cma-command-id"], receipt);
    let bytes = to_bytes(replay.into_body(), 1024 * 1024).await.unwrap();
    let replay = events(&bytes);
    assert_eq!(replay[3], reported);
    assert_eq!(replay[4]["item"]["status"], "completed");
    assert_eq!(replay.last().unwrap()["response"], after);
    for (sequence, event) in replay.iter().enumerate() {
        assert_eq!(event["sequence_number"], sequence);
    }
    let items = fixture.read(&items_path).await;
    let tool = items["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "cma_chat.tool_activity")
        .unwrap();
    assert_eq!(tool["output"], reported["item"]["output"]);
    assert_eq!(tool["status"], "completed");
    assert_eq!(fixture.dispatches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn legacy_output_requires_recorded_context_and_omitted_output_stays_absent() {
    let mut fixture = fixture().await;
    let response = fixture.create().await;
    for update in [
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"unreported-command", "content":[{"type":"content", "content":{"type":"text", "text":"no recorded status"}}]}),
        json!({"sessionUpdate":"tool_call", "toolCallId":"command-proof", "title":"printf original", "kind":"execute", "status":"in_progress"}),
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"command-proof", "title":"printf revised", "status":"failed"}),
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"command-proof", "content":[{"type":"content", "content":{"type":"text", "text":"recorded failure output"}}]}),
        json!({"end_turn":true}),
    ] {
        fixture.update(update).await;
    }
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let events = events(&bytes);
    let activities: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.cma_chat.tool_activity")
        .collect();
    assert_eq!(activities.len(), 3);
    for activity in &activities[..2] {
        assert!(activity["item"].get("output").is_none());
        assert_eq!(activity["item"]["id"], activities[0]["item"]["id"]);
    }
    assert_eq!(activities[0]["item"]["status"], "in_progress");
    assert_eq!(activities[0]["item"]["target"], "printf original");
    assert_eq!(activities[2]["item"]["status"], "failed");
    assert_eq!(activities[2]["item"]["target"], "printf revised");
    assert_eq!(activities[2]["item"]["kind"], "execute");
    assert_eq!(
        activities[2]["item"]["output"],
        json!({"text":"recorded failure output", "truncated":false})
    );
    let items = fixture
        .read(&format!("/api/v1/conversations/{AGENT}/items"))
        .await;
    assert_eq!(items["data"][0], activities[2]["item"]);
    assert!(!String::from_utf8(bytes.to_vec())
        .unwrap()
        .contains("unreported-command"));
}
