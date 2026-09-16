use super::*;
use axum::body::{to_bytes, Body};
use futures_util::StreamExt;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::AgentLogEntry;
use tower::ServiceExt;

const INPUT: &str = "sdk-raw-with-tools";

#[derive(Clone, Copy, Debug)]
pub(super) enum Route {
    Agui,
    Responses,
    ResponsesV2,
}

impl Route {
    pub(super) fn request(self, agent: &AgentId, input: &str) -> Request<Body> {
        let (path, body) = match self {
            Self::Agui => (
                "/api/v1/agui/run",
                json!({"threadId":agent,"messages":[{"role":"user","content":input}]}),
            ),
            Self::Responses => (
                "/api/v1/responses",
                json!({"conversation":agent,"input":input,"stream":true}),
            ),
            Self::ResponsesV2 => (
                "/api/v2/responses",
                json!({"conversation":agent,"input":input,"stream":true}),
            ),
        };
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", TEST_BEARER)
            .header("content-type", "application/json")
            .header("idempotency-key", "retained-public-input")
            .body(Body::from(body.to_string()))
            .unwrap()
    }
}

pub(super) fn authenticated<S>(router: Router, harness: &Harness<S>) -> Router {
    router.layer(middleware::from_fn_with_state(
        harness.workspace.owner.clone(),
        authenticate,
    ))
}

pub(super) async fn read(router: &Router, path: &str) -> String {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", TEST_BEARER)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(response.into_body(), 4 * 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(status, StatusCode::OK, "{path}: {text}");
    text
}

pub(super) async fn admitted(harness: &Harness) -> AdmittedAgentCommand {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let page = harness
                .store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap();
            if let Some(command) = page.items.into_iter().next() {
                return command;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the actual public handler durably admits its request")
}

fn semantic_output(logs: &[AgentLogEntry]) -> Vec<Value> {
    logs.iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::OwnerMessage { content } => Some(json!({"input":content})),
            AgentLogKind::AgentMessage { content } => Some(json!({"output":content})),
            AgentLogKind::ToolCallStarted { .. } | AgentLogKind::ToolCallUpdated { .. } => {
                Some(serde_json::to_value(&entry.kind).unwrap())
            }
            AgentLogKind::TurnCompleted { .. } => Some(json!({"completed":true})),
            _ => None,
        })
        .collect()
}

async fn exercise(route: Route) -> Vec<Value> {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("public-outcome", vec![]).await;
    let agent = harness.drive_ready(&agent.id).await;
    let probe = runtime_tests::RuntimeProbe::gated();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "public-first-worker").await;
    let router = authenticated(router, &harness);
    let request = route.request(&agent.id, INPUT);
    let pending = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    let original = admitted(&harness).await;
    let id = &original.receipt.envelope.command_id;
    assert_eq!(original.receipt.envelope.source.kind, AgentSourceKind::Api);
    worker.run_one(id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{route:?}");
    assert_eq!(response.headers()["x-cma-command-id"], id.as_str());
    let response_id = original.receipt.response_id.as_ref().unwrap();
    assert_eq!(
        response.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&first).unwrap().contains("data:"));
    drop(stream);
    drop(worker);

    // A fresh execution adapter and worker use the original recorded dispatch.
    // No recorder update has been released yet, so no completion can mask a resend.
    let (reopened, replacement) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "public-replacement-worker")
            .await;
    let reopened = authenticated(reopened, &harness);
    let bound = harness.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, original.receipt);
    assert!(bound.work.dispatch.is_some());
    assert_eq!(bound.status, AgentResponseStatus::InProgress);
    replacement.run_one(id).await.unwrap();
    let writes = probe.inputs();
    assert_eq!(
        writes.len(),
        1,
        "{route:?} must not resend on worker replacement"
    );
    assert_eq!(writes[0].0.agent_id, agent.id);
    assert_eq!(
        writes[0].0.conversation_id,
        agent
            .conversation
            .conversation_id
            .as_ref()
            .unwrap()
            .as_str()
    );
    assert!(
        matches!(&writes[0].1, ConversationInput::RecoverablePrompt { content: blocks, .. }
        if blocks.len() == 1 && matches!(&blocks[0], layer3_adapter::ContentBlock::Text(text) if text.text == INPUT))
    );
    probe.release_reply();

    let response_path = format!("/api/v2/responses/{response_id}");
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response: Value =
                serde_json::from_str(&read(&reopened, &response_path).await).unwrap();
            if response["status"] == "completed" {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual recorder output completes the disconnected command");
    assert_eq!(completed["id"], response_id.as_str());
    assert_eq!(completed["conversation"]["id"], agent.id.as_str());
    assert_eq!(
        completed["output"][0]["content"][0]["text"],
        "Hello from CMA"
    );
    // Public reads can project the committed runtime outcome before the command
    // worker persists its reconciliation. Recover that completion without input.
    replacement.run_one(id).await.unwrap();
    let done = harness.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.work.dispatch, bound.work.dispatch);
    assert_eq!(done.status, AgentResponseStatus::Completed);

    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::AgentMessage { .. }))
            .count(),
        1
    );
    assert!(logs.iter().any(|e| matches!(&e.kind, AgentLogKind::ToolCallStarted { call_id, .. } if call_id == "sdk-proof")));
    assert!(logs.iter().any(|e| matches!(&e.kind, AgentLogKind::ToolCallUpdated { call_id, status, .. } if call_id == "sdk-proof" && status == "completed")));
    assert!(!serde_json::to_string(&logs)
        .unwrap()
        .contains("Checking the proof"));

    // All reconnects below are GETs. Each public projection reads the output
    // recorded from the runtime, and no test manufactures event or response DTOs.
    let events_path = format!("/api/v2/responses/{response_id}/events");
    let events = read(&reopened, &events_path).await;
    assert!(events.contains("response.completed") && events.contains("Hello from CMA"));
    assert_eq!(read(&reopened, &events_path).await, events);
    let agent_events = read(
        &reopened,
        &format!("/api/v2/agents/{}/events/snapshot", agent.id),
    )
    .await;
    assert!(agent_events.contains("sdk-proof") && agent_events.contains("Hello from CMA"));
    let agui_path = format!("/api/v1/agui/events?thread_id={}&follow=false", agent.id);
    let agui = read(&reopened, &agui_path).await;
    assert!(agui.contains("RUN_FINISHED") && agui.contains("Hello from CMA"));
    assert_eq!(read(&reopened, &agui_path).await, agui);
    let turn = done.work.dispatch.as_ref().unwrap().turn_sequence;
    let legacy = read(
        &reopened,
        &format!("/api/v1/responses/resp_{}_{turn}", agent.id),
    )
    .await;
    assert!(legacy.contains("Hello from CMA"));
    for wire in [&events, &agent_events, &agui, &legacy] {
        assert!(!wire.contains("Checking the proof"));
    }
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );

    // Explicit keyed recovery is independently idempotent after read-only replay.
    let recovered = reopened
        .oneshot(route.request(&agent.id, INPUT))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK, "{route:?}");
    assert_eq!(recovered.headers()["x-cma-command-id"], id.as_str());
    assert_eq!(
        recovered.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(recovered);
    replacement.run_one(id).await.unwrap();
    assert_eq!(probe.inputs().len(), 1, "{route:?}");
    assert_eq!(
        harness
            .store
            .get_command(id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        original.receipt
    );
    semantic_output(&logs)
}

#[tokio::test]
async fn public_text_and_tool_outcomes_survive_disconnect_and_worker_replacement_without_resend() {
    let expected = exercise(Route::Agui).await;
    assert_eq!(
        expected.len(),
        5,
        "one input, tool start/update, answer and completion"
    );
    for route in [Route::Responses, Route::ResponsesV2] {
        assert_eq!(exercise(route).await, expected, "{route:?}");
    }
}
