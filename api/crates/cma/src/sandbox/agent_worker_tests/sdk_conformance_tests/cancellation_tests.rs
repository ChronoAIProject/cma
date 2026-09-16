use super::cross_interface_tests::{admitted, authenticated, read, Route};
use super::*;
use axum::body::{to_bytes, Body};
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::*;
use tower::ServiceExt;

const INPUT: &str = "Explain the result until interrupted.";
const PARTIAL: &str = "Partial output before cancellation";

async fn cancel(router: &Router, path: &str, intent: &Value) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", TEST_BEARER)
                .header("content-type", "application/json")
                .header("idempotency-key", "cancel-public-input")
                .body(Body::from(intent.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::ACCEPTED, "{value}");
    value
}

async fn exercise(route: Route) -> Vec<Value> {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("public-runtime-cancellation", vec![]).await;
    let agent = harness.drive_ready(&agent.id).await;
    assert!(agent.capabilities.supports(RuntimeCapability::Interrupt));
    let probe = runtime_tests::RuntimeProbe::interruptible();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "cancel-first-worker").await;
    let router = authenticated(router, &harness);
    let request = route.request(&agent.id, INPUT);
    let admitting = router.clone();
    let pending = tokio::spawn(async move { admitting.oneshot(request).await.unwrap() });
    let original = admitted(&harness).await;
    let source_id = &original.receipt.envelope.command_id;
    let response_id = original.receipt.response_id.as_ref().unwrap();
    worker.run_one(source_id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{route:?}");
    assert_eq!(response.headers()["x-cma-command-id"], source_id.as_str());
    assert_eq!(
        response.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(response);
    let bound = harness.store.get_command(source_id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, original.receipt);
    assert_eq!(bound.status, AgentResponseStatus::InProgress);
    let turn = bound.work.dispatch.as_ref().unwrap().turn_sequence;
    let path = format!(
        "/api/v2/agents/{}/responses/{response_id}/controls",
        agent.id
    );
    let observed: Value = serde_json::from_str(&read(&router, &path).await).unwrap();
    assert_eq!(observed["target_active"], true);
    assert_eq!(observed["target"]["turn_sequence"], turn);
    let intent = json!({
        "turn_sequence": observed["target"]["turn_sequence"],
        "expected_agent_revision": observed["agent_revision"],
        "command": {"type": "cancel"}
    });
    let accepted = cancel(&router, &path, &intent).await;
    let control_id = AgentCommandId(
        accepted["control"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    let control = harness
        .store
        .get_command(&control_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(control.receipt.envelope.command, AgentCommand::Interrupt);
    assert_eq!(control.receipt.response_id, None);
    worker.run_one(&control_id).await.unwrap();
    let delivered = harness
        .store
        .get_command(&control_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered.receipt, control.receipt);
    assert_eq!(delivered.status, AgentResponseStatus::Completed);
    let observed: Value = serde_json::from_str(&read(&router, &path).await).unwrap();
    assert_eq!(observed["response"]["status"], "in_progress");
    assert_eq!(observed["target_active"], true);
    assert_eq!(
        harness.stored(&agent.id).await.conversation.active_turn,
        Some(turn)
    );
    assert!(!harness
        .store
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::TurnInterrupted { .. })));
    let writes = probe.inputs();
    assert_eq!(writes.len(), 2);
    assert!(
        matches!(&writes[0].1, ConversationInput::RecoverablePrompt { content: blocks, .. }
        if blocks.len() == 1 && matches!(&blocks[0], layer3_adapter::ContentBlock::Text(text) if text.text == INPUT))
    );
    assert!(matches!(writes[1].1, ConversationInput::Cancel));
    assert_eq!(writes[0].0, writes[1].0);
    drop(worker);
    drop(router);

    // Both writes have returned before replacing the worker. A delivered cancel
    // is not a cancelled source until the real recorder sees the runtime outcome.
    let (reopened, replacement) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "cancel-replacement-worker")
            .await;
    let reopened = authenticated(reopened, &harness);
    replacement.run_one(source_id).await.unwrap();
    replacement.run_one(&control_id).await.unwrap();
    let recovered_control = cancel(&reopened, &path, &intent).await;
    assert_eq!(
        recovered_control["control"]["receipt"],
        accepted["control"]["receipt"]
    );
    assert_eq!(recovered_control["control"]["status"], "completed");
    assert_eq!(
        probe.inputs().len(),
        2,
        "{route:?} must not resend either input"
    );
    probe.release_reply();

    let response_path = format!("/api/v2/responses/{response_id}");
    let cancelled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value =
                serde_json::from_str(&read(&reopened, &response_path).await).unwrap();
            if value["status"] == "incomplete" {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the recorder confirms runtime cancellation");
    assert_eq!(cancelled["incomplete_details"]["reason"], "interrupted");
    assert_eq!(cancelled["output"][0]["content"][0]["text"], PARTIAL);
    replacement.run_one(source_id).await.unwrap();
    let done = harness.store.get_command(source_id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Cancelled);
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.work.dispatch, bound.work.dispatch);
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.id, agent.id);
    assert_eq!(current.status, AgentStatus::Ready);
    assert_eq!(current.conversation.active_turn, None);
    let observed: Value = serde_json::from_str(&read(&reopened, &path).await).unwrap();
    assert_eq!(observed["response"]["status"], "cancelled");
    assert_eq!(observed["target_active"], false);

    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnInterrupted { .. }))
            .count(),
        1
    );
    assert!(!logs.iter().any(|entry| matches!(
        entry.kind,
        AgentLogKind::TurnCompleted { .. } | AgentLogKind::TurnFailed { .. }
    )));
    let semantic = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::OwnerMessage { content } => Some(json!({"input":content})),
            AgentLogKind::AgentMessage { content } => Some(json!({"output":content})),
            AgentLogKind::TurnInterrupted { .. } => Some(json!({"interrupted":true})),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        semantic.len(),
        3,
        "one input, partial output and cancellation"
    );
    let legacy: Value = serde_json::from_str(
        &read(
            &reopened,
            &format!("/api/v1/responses/resp_{}_{turn}", agent.id),
        )
        .await,
    )
    .unwrap();
    assert_eq!(legacy["status"], "incomplete");
    assert_eq!(
        legacy["incomplete_details"],
        cancelled["incomplete_details"]
    );
    assert_eq!(legacy["output"][0]["content"][0]["text"], PARTIAL);
    let v2_path = format!("/api/v2/responses/{response_id}/events");
    let replay = read(&reopened, &v2_path).await;
    assert!(replay.contains("response.incomplete") && replay.contains(PARTIAL));
    assert!(!replay.contains("response.completed") && !replay.contains("response.failed"));
    assert_eq!(read(&reopened, &v2_path).await, replay);
    let agui_path = format!("/api/v1/agui/events?thread_id={}&follow=false", agent.id);
    let agui = read(&reopened, &agui_path).await;
    assert!(
        agui.contains("RUN_FINISHED")
            && agui.contains("\"type\":\"interrupt\"")
            && agui.contains(PARTIAL)
    );
    assert_eq!(read(&reopened, &agui_path).await, agui);
    let snapshot = read(
        &reopened,
        &format!("/api/v2/agents/{}/events/snapshot", agent.id),
    )
    .await;
    assert!(snapshot.contains("turn_cancelled") && snapshot.contains(PARTIAL));
    for wire in [&replay, &agui, &snapshot] {
        assert!(!wire.contains("Checking the proof"));
    }
    assert!(!serde_json::to_string(&logs)
        .unwrap()
        .contains("Checking the proof"));
    let recovered = reopened
        .clone()
        .oneshot(route.request(&agent.id, INPUT))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(recovered.headers()["x-cma-command-id"], source_id.as_str());
    assert_eq!(
        recovered.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(recovered);
    assert_eq!(
        cancel(&reopened, &path, &intent).await["control"]["receipt"],
        accepted["control"]["receipt"]
    );
    replacement.run_one(source_id).await.unwrap();
    replacement.run_one(&control_id).await.unwrap();
    assert_eq!(
        probe.inputs().len(),
        2,
        "{route:?} retains exactly one prompt and one cancel"
    );
    assert_eq!(
        harness
            .store
            .get_command(source_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        original.receipt
    );
    assert_eq!(
        harness
            .store
            .get_command(&control_id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        control.receipt
    );
    assert_eq!(
        harness
            .store
            .get_command(&control_id)
            .await
            .unwrap()
            .unwrap()
            .work
            .dispatch,
        delivered.work.dispatch
    );
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );
    semantic
}

#[tokio::test]
async fn public_cancellation_requires_runtime_outcome_and_replays_without_resending() {
    let expected = exercise(Route::Agui).await;
    for route in [Route::Responses, Route::ResponsesV2] {
        assert_eq!(exercise(route).await, expected, "{route:?}");
    }
}
