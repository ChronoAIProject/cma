use super::cross_interface_tests::{admitted, authenticated, read, Route};
use super::*;
use axum::body::to_bytes;
use layer2_core::sandbox::agent_io::*;
use tower::ServiceExt;

const INPUT: &str = "Explain the retained result.";
const PARTIAL: &str = "Partial output before failure";
const FAILURE: &str = "agent_turn_failed";
const PRIVATE_FAILURE: &str = "fixture_turn_failed";
const PRIVATE_DETAIL: &str = "The runtime failed after producing partial output.";
const PUBLIC_DETAIL: &str = "The Agent run failed. The failed turn will not be replayed automatically. Earlier work may have completed; review the conversation before sending a new message to continue.";

fn assert_public_failure(wire: &str) {
    assert!(
        wire.contains(FAILURE) && wire.contains(PUBLIC_DETAIL),
        "{wire}"
    );
    assert!(
        !wire.contains(PRIVATE_FAILURE) && !wire.contains(PRIVATE_DETAIL),
        "{wire}"
    );
}

async fn body(response: Response) -> String {
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(response.into_body(), 4 * 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn exercise(route: Route) -> Vec<Value> {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("public-runtime-failure", vec![]).await;
    let agent = harness.drive_ready(&agent.id).await;
    let probe = runtime_tests::RuntimeProbe::failing();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "failure-first-worker").await;
    let router = authenticated(router, &harness);
    let request = route.request(&agent.id, INPUT);
    let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    let original = admitted(&harness).await;
    let command_id = &original.receipt.envelope.command_id;
    let response_id = original.receipt.response_id.as_ref().unwrap();
    worker.run_one(command_id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{route:?}");
    assert_eq!(response.headers()["x-cma-command-id"], command_id.as_str());
    assert_eq!(
        response.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    let bound = harness
        .store
        .get_command(command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.receipt, original.receipt);
    assert!(bound.work.dispatch.is_some());
    assert_eq!(bound.status, AgentResponseStatus::InProgress);
    assert_eq!(probe.inputs().len(), 1);
    drop(worker);

    // Failure comes from actual runtime updates through the durable recorder,
    // after input was accepted. No queue rejection or log row is manufactured.
    probe.release_reply();
    let live = body(response).await;
    assert_public_failure(&live);
    assert!(live.contains("Partial output ") && live.contains("before failure"));
    match route {
        Route::Agui => assert!(live.contains("RUN_ERROR"), "{live}"),
        Route::Responses | Route::ResponsesV2 => {
            assert!(live.contains("response.failed"), "{live}");
            assert!(!live.contains("response.completed"));
        }
    }

    let (reopened, replacement) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "failure-replacement-worker")
            .await;
    let reopened = authenticated(reopened, &harness);
    replacement.run_one(command_id).await.unwrap();
    let failed = harness
        .store
        .get_command(command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentResponseStatus::Failed);
    assert_eq!(failed.receipt, original.receipt);
    assert_eq!(failed.work.dispatch, bound.work.dispatch);
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.id, agent.id);
    assert_eq!(current.status, AgentStatus::Ready);
    assert_eq!(current.conversation.active_turn, None);
    let writes = probe.inputs();
    assert_eq!(writes.len(), 1, "{route:?}");
    assert_eq!(writes[0].0.agent_id, agent.id);
    assert!(
        matches!(&writes[0].1, ConversationInput::RecoverablePrompt { content: blocks, .. }
        if blocks.len() == 1 && matches!(&blocks[0], layer3_adapter::ContentBlock::Text(text) if text.text == INPUT))
    );

    let response_path = format!("/api/v2/responses/{response_id}");
    let retrieved: Value = serde_json::from_str(&read(&reopened, &response_path).await).unwrap();
    assert_eq!(retrieved["id"], response_id.as_str());
    assert_eq!(retrieved["status"], "failed");
    assert_eq!(retrieved["error"]["code"], FAILURE);
    assert_eq!(retrieved["error"]["message"], PUBLIC_DETAIL);
    assert_public_failure(&retrieved.to_string());
    assert_eq!(retrieved["output"][0]["content"][0]["text"], PARTIAL);
    let turn = failed.work.dispatch.as_ref().unwrap().turn_sequence;
    let legacy: Value = serde_json::from_str(
        &read(
            &reopened,
            &format!("/api/v1/responses/resp_{}_{turn}", agent.id),
        )
        .await,
    )
    .unwrap();
    assert_eq!(legacy["status"], "failed");
    assert_eq!(legacy["error"], retrieved["error"]);
    assert_public_failure(&legacy.to_string());
    assert_eq!(legacy["output"][0]["content"][0]["text"], PARTIAL);

    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    assert!(!logs
        .iter()
        .any(|e| matches!(e.kind, AgentLogKind::TurnCompleted { .. })));
    assert!(logs.iter().any(|entry| matches!(&entry.kind,
        AgentLogKind::TurnFailed { failure, .. }
        if failure.class == PRIVATE_FAILURE && failure.detail == PRIVATE_DETAIL)));
    assert!(!serde_json::to_string(&logs)
        .unwrap()
        .contains("Checking the proof"));
    let semantic = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::OwnerMessage { content } => Some(json!({"input":content})),
            AgentLogKind::AgentMessage { content } => Some(json!({"output":content})),
            AgentLogKind::TurnFailed { failure, .. } => Some(json!({"failure":failure})),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        semantic.len(),
        3,
        "one input, retained partial output and failure"
    );

    let v2_path = format!("/api/v2/responses/{response_id}/events");
    let replay = read(&reopened, &v2_path).await;
    assert!(replay.contains("response.failed") && replay.contains(PARTIAL));
    assert!(!replay.contains("response.completed"));
    assert_eq!(read(&reopened, &v2_path).await, replay);
    let agui_path = format!("/api/v1/agui/events?thread_id={}&follow=false", agent.id);
    let agui = read(&reopened, &agui_path).await;
    assert!(agui.contains("RUN_ERROR") && agui.contains(FAILURE) && agui.contains(PARTIAL));
    assert_eq!(read(&reopened, &agui_path).await, agui);
    let snapshot = read(
        &reopened,
        &format!("/api/v2/agents/{}/events/snapshot", agent.id),
    )
    .await;
    assert!(snapshot.contains(FAILURE) && snapshot.contains(PARTIAL));
    for wire in [&replay, &agui, &snapshot] {
        assert_public_failure(wire);
        assert!(!wire.contains("Checking the proof"));
    }
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );

    let recovered = reopened
        .oneshot(route.request(&agent.id, INPUT))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK, "{route:?}");
    assert_eq!(recovered.headers()["x-cma-command-id"], command_id.as_str());
    assert_eq!(
        recovered.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    assert_public_failure(&body(recovered).await);
    replacement.run_one(command_id).await.unwrap();
    assert_eq!(
        probe.inputs().len(),
        1,
        "{route:?} never resends a failed turn"
    );
    assert_eq!(
        harness
            .store
            .get_command(command_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        original.receipt
    );
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );
    semantic
}

#[tokio::test]
async fn public_runtime_failure_preserves_partial_output_and_replays_without_resending() {
    let expected = exercise(Route::Agui).await;
    for route in [Route::Responses, Route::ResponsesV2] {
        assert_eq!(exercise(route).await, expected, "{route:?}");
    }
}
