use super::fixtures::*;
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::{
    sandbox::{
        AgentMessageAssembly, AgentRuntimeUpdate, AgentUpdateRecorder, DurableAgentUpdateRecorder,
    },
    AgentId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn reported_output_crosses_durable_snapshot_tail_and_response_boundaries_without_status_invention(
) {
    let fixture = fixture(true, false).await;
    let recorder =
        DurableAgentUpdateRecorder::with_writer_policy(fixture.store.clone(), task_policy());
    let mut assembly = AgentMessageAssembly::default();
    for update in [
        json!({"sessionUpdate":"tool_call", "toolCallId":"output-proof", "title":"printf native-computer-proof", "kind":"execute", "status":"in_progress"}),
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"output-proof", "content":[{"type":"content", "content":{"type":"text", "text":"partial proof"}}]}),
        json!({"sessionUpdate":"tool_call_update", "toolCallId":"output-proof", "status":"completed", "rawOutput":{"formatted_output":"native-computer-proof\n", "exit_code":0}}),
    ] {
        recorder
            .record_update(
                &AgentId("agt_fixture".into()),
                1,
                &mut assembly,
                AgentRuntimeUpdate::Said(serde_json::from_value(update).unwrap()),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let mut authorized = request(Method::GET, SNAPSHOT, "owner-a", None, None);
    authorized
        .headers_mut()
        .insert("prefer", "cma-tool-output".parse().unwrap());
    let response = fixture.router.clone().oneshot(authorized).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let kinds: Vec<&Value> = snapshot["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["kind"])
        .collect();
    let partial = kinds
        .iter()
        .find(|kind| kind["type"] == "tool_output_reported")
        .unwrap();
    assert_eq!(
        partial["output"],
        json!({"text":"partial proof", "truncated":false})
    );
    assert!(partial.get("status").is_none());
    assert!(
        kinds
            .iter()
            .any(|kind| kind["output"]
                == json!({"text":"native-computer-proof\n", "truncated":false}))
    );
    let (_, legacy) = call(
        &fixture.router,
        Method::GET,
        SNAPSHOT,
        "owner-a",
        None,
        None,
    )
    .await;
    assert!(!legacy.to_string().contains("partial proof"));
    assert!(!legacy.to_string().contains("native-computer-proof\\n"));
    assert_eq!(legacy["through_cursor"], snapshot["through_cursor"]);
    let (status, response) = call(
        &fixture.router,
        Method::GET,
        "/api/v2/responses/resp_control_source",
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(response["output"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "mcp_call"
            && item["cma:output"] == json!({"text":"native-computer-proof\n", "truncated":false})
            && item["output"].is_null()));
    let mut tail = request(
        Method::GET,
        &format!("{EVENTS}?follow=false"),
        "owner-a",
        None,
        None,
    );
    tail.headers_mut()
        .insert("prefer", "cma-tool-output".parse().unwrap());
    let response = fixture.router.clone().oneshot(tail).await.unwrap();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("native-computer-proof\\n"));
    let (status, denied) = call(
        &fixture.router,
        Method::GET,
        SNAPSHOT,
        "outsider",
        None,
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(!denied.to_string().contains("native-computer-proof"));
}
