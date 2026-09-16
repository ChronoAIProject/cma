use super::cross_interface_tests::{admitted, read, Route};
use super::runtime_tests::{APPROVAL_ID, APPROVAL_LOCATOR};
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer3_adapter::PermissionDecision;
use tower::ServiceExt;

mod fixtures;
use fixtures::*;

const INPUT: &str = "Read the file after my approval.";

async fn exercise(route: Route) -> Vec<Value> {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("public-approval", vec![]).await;
    let agent = harness.drive_ready(&agent.id).await;
    let probe = runtime_tests::RuntimeProbe::approving();
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "approval-first-worker")
            .await;
    let router = authenticated(router, route, &harness);
    let pending = tokio::spawn({
        let router = router.clone();
        let request = route.request(&agent.id, INPUT);
        async move { router.oneshot(request).await.unwrap() }
    });
    let original = admitted(&harness).await;
    let source_id = &original.receipt.envelope.command_id;
    let response_id = original.receipt.response_id.as_ref().unwrap();
    assert_eq!(
        original.receipt.envelope.actor,
        context(route, &harness).actor
    );
    let source_kind = match route {
        Route::Agui => AgentSourceKind::UserInterface,
        _ => AgentSourceKind::Api,
    };
    assert_eq!(original.receipt.envelope.source.kind, source_kind);
    assert_eq!(
        original.receipt.envelope.owner,
        AgentOwner::Person {
            subject: harness.workspace.owner.clone()
        }
    );
    worker.run_one(source_id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-cma-command-id"], source_id.as_str());
    assert_eq!(
        response.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(response);
    probe.release_reply();
    tokio::time::timeout(Duration::from_secs(5), async {
        while harness.stored(&agent.id).await.status != AgentStatus::WaitingApproval {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual recorder exposes the runtime approval request");
    worker.run_one(source_id).await.unwrap();
    let waiting = harness.store.get_command(source_id).await.unwrap().unwrap();
    assert_eq!(waiting.status, AgentResponseStatus::WaitingAction);
    assert_eq!(waiting.receipt, original.receipt);
    let turn = waiting.work.dispatch.as_ref().unwrap().turn_sequence;
    let approvals = harness
        .store
        .pending_agent_approvals(&agent.id)
        .await
        .unwrap();
    assert_eq!(approvals.len(), 1);
    assert!(
        matches!(&approvals[0].kind, AgentLogKind::ApprovalRequested { approval_id, runtime_locator:Some(locator), .. } if approval_id == APPROVAL_ID && locator == APPROVAL_LOCATOR)
    );
    let pending_snapshot = snapshot(&router, &agent.id).await;
    assert_approval_binding(&pending_snapshot, "approval_requested", &waiting);
    let agui_path = format!("/api/v1/agui/events?thread_id={}&follow=false", agent.id);
    let pending_replay = read(&router, &agui_path).await;
    assert!(pending_replay.contains(APPROVAL_ID) && pending_replay.contains("\"op\":\"add\""));
    assert!(!pending_replay.contains(APPROVAL_LOCATOR));
    assert_eq!(probe.inputs().len(), 1);
    drop(worker);
    drop(router);

    // Reconstruct services while the approval is pending. The durable locator
    // and original source binding must suffice; no new approval or turn is seeded.
    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "approval-pending-worker")
            .await;
    let router = authenticated(router, route, &harness);
    worker.run_one(source_id).await.unwrap();
    assert_eq!(snapshot(&router, &agent.id).await, pending_snapshot);
    let source_replay = router
        .clone()
        .oneshot(route.request(&agent.id, INPUT))
        .await
        .unwrap();
    assert_eq!(source_replay.status(), StatusCode::OK);
    assert_eq!(
        source_replay.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(source_replay);
    assert_eq!(probe.inputs().len(), 1);
    let path = format!(
        "/api/v2/agents/{}/responses/{response_id}/controls",
        agent.id
    );
    let observed: Value = serde_json::from_str(&read(&router, &path).await).unwrap();
    assert_eq!(observed["response"]["status"], "waiting_action");
    assert_eq!(observed["target_active"], true);
    assert_eq!(observed["runtime_support"]["resolve_approval"], true);
    let intent = json!({"turn_sequence":turn,"expected_agent_revision":observed["agent_revision"],
        "command":{"type":"resolve_approval","approval_id":APPROVAL_ID,"decision":"allow"}});
    if !matches!(route, Route::Agui) {
        wrong_approval_is_refused(&router, &path, &intent).await;
        assert_eq!(
            harness
                .store
                .pending_agent_approvals(&agent.id)
                .await
                .unwrap(),
            approvals
        );
        assert_eq!(probe.inputs().len(), 1);
    }
    let pending = tokio::spawn({
        let router = router.clone();
        let request = answer_request(route, &agent.id, &path, &intent);
        async move { router.oneshot(request).await.unwrap() }
    });
    let answer = answer_admitted(&harness).await;
    let answer_id = &answer.receipt.envelope.command_id;
    assert_eq!(answer.receipt.response_id, None);
    assert_eq!(
        answer.receipt.envelope.actor,
        original.receipt.envelope.actor
    );
    assert_eq!(
        answer.receipt.envelope.owner,
        original.receipt.envelope.owner
    );
    assert_eq!(answer.receipt.envelope.source.kind, source_kind);
    assert_eq!(
        answer.receipt.envelope.command,
        AgentCommand::AnswerApproval {
            approval_id: APPROVAL_ID.into(),
            decision: layer2_core::sandbox::AgentApprovalDecision::Allow
        }
    );
    worker.run_one(answer_id).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    assert_answer_response(response, route, &answer).await;
    let delivered = harness.store.get_command(answer_id).await.unwrap().unwrap();
    assert_eq!(delivered.receipt, answer.receipt);
    assert_eq!(delivered.status, AgentResponseStatus::Completed);
    assert_eq!(
        delivered.work.dispatch.as_ref().unwrap().turn_sequence,
        turn
    );
    worker.run_one(source_id).await.unwrap();
    let active = harness.store.get_command(source_id).await.unwrap().unwrap();
    assert_eq!(active.status, AgentResponseStatus::InProgress);
    assert_eq!(active.receipt, original.receipt);
    assert_eq!(active.work.dispatch, waiting.work.dispatch);
    assert!(harness
        .store
        .pending_agent_approvals(&agent.id)
        .await
        .unwrap()
        .is_empty());
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.status, AgentStatus::Working);
    assert_eq!(current.conversation.active_turn, Some(turn));
    // ApprovalResolved records the owner's decision. Delivery is not evidence
    // of source-turn completion; the runtime output remains gated below.
    assert!(!harness
        .store
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|e| matches!(e.kind, AgentLogKind::TurnCompleted { .. })));
    let resolved_snapshot = snapshot(&router, &agent.id).await;
    assert_approval_binding(&resolved_snapshot, "approval_requested", &waiting);
    assert_approval_binding(&resolved_snapshot, "approval_resolved", &waiting);
    let writes = probe.inputs();
    assert_eq!(writes.len(), 2);
    assert!(
        matches!(&writes[0].1, ConversationInput::Prompt(blocks) if blocks.len() == 1 && matches!(&blocks[0], layer3_adapter::ContentBlock::Text(text) if text.text == INPUT))
    );
    assert!(
        matches!(&writes[1].1, ConversationInput::PermissionAnswer { id, locator:Some(locator), decision:PermissionDecision::Allow } if id == APPROVAL_ID && locator.as_str() == APPROVAL_LOCATOR)
    );
    assert_eq!(writes[0].0, writes[1].0);
    drop(worker);
    drop(router);

    let (router, worker) =
        conversation_router_with_probe(&harness, Some(probe.clone()), "approval-delivered-worker")
            .await;
    let router = authenticated(router, route, &harness);
    worker.run_one(source_id).await.unwrap();
    worker.run_one(answer_id).await.unwrap();
    let recovered = router
        .clone()
        .oneshot(answer_request(route, &agent.id, &path, &intent))
        .await
        .unwrap();
    assert_answer_response(recovered, route, &answer).await;
    assert_eq!(probe.inputs().len(), 2);
    probe.release_reply();
    let response_path = format!("/api/v2/responses/{response_id}");
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value: Value = serde_json::from_str(&read(&router, &response_path).await).unwrap();
            if value["status"] == "completed" {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual recorder completes the original source after approval");
    assert_eq!(completed["id"], response_id.as_str());
    assert_eq!(
        completed["output"][0]["content"][0]["text"],
        "Hello from CMA"
    );
    worker.run_one(source_id).await.unwrap();
    let done = harness.store.get_command(source_id).await.unwrap().unwrap();
    assert_eq!(done.status, AgentResponseStatus::Completed);
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.work.dispatch, waiting.work.dispatch);
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.status, AgentStatus::Ready);
    assert_eq!(current.conversation.active_turn, None);
    let logs = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    let semantic: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::OwnerMessage { content } => Some(json!({"input":content})),
            AgentLogKind::ApprovalRequested {
                approval_id,
                action,
                detail,
                ..
            } => Some(json!({"approval":approval_id,"action":action,"detail":detail})),
            AgentLogKind::ApprovalResolved {
                approval_id,
                decision,
            } => Some(json!({"resolved":approval_id,"decision":decision})),
            AgentLogKind::AgentMessage { content } => Some(json!({"output":content})),
            AgentLogKind::TurnCompleted { .. } => Some(json!({"completed":true})),
            _ => None,
        })
        .collect();
    assert_eq!(
        semantic.len(),
        5,
        "one prompt, request, decision, answer and completion"
    );
    let rich = snapshot(&router, &agent.id).await;
    assert_approval_binding(&rich, "approval_requested", &done);
    assert_approval_binding(&rich, "approval_resolved", &done);
    assert_eq!(snapshot(&router, &agent.id).await, rich);
    let agui = read(&router, &agui_path).await;
    assert!(
        agui.contains(APPROVAL_ID)
            && agui.contains("\"op\":\"remove\"")
            && agui.contains("Hello from CMA")
    );
    assert_eq!(read(&router, &agui_path).await, agui);
    let events = read(&router, &format!("/api/v2/responses/{response_id}/events")).await;
    assert!(events.contains("response.completed") && events.contains("Hello from CMA"));
    let legacy: Value = serde_json::from_str(
        &read(
            &router,
            &format!("/api/v1/responses/resp_{}_{turn}", agent.id),
        )
        .await,
    )
    .unwrap();
    assert_eq!(legacy["status"], "completed");
    for wire in [rich.to_string(), agui, events, legacy.to_string()] {
        assert!(!wire.contains(APPROVAL_LOCATOR) && !wire.contains("Checking the proof"));
    }
    let source_replay = router
        .clone()
        .oneshot(route.request(&agent.id, INPUT))
        .await
        .unwrap();
    assert_eq!(
        source_replay.headers()["x-cma-command-id"],
        source_id.as_str()
    );
    assert_eq!(
        source_replay.headers()["x-cma-response-id"],
        response_id.as_str()
    );
    drop(source_replay);
    let recovered = router
        .clone()
        .oneshot(answer_request(route, &agent.id, &path, &intent))
        .await
        .unwrap();
    assert_answer_response(recovered, route, &answer).await;
    worker.run_one(source_id).await.unwrap();
    worker.run_one(answer_id).await.unwrap();
    assert_eq!(probe.inputs().len(), 2);
    let retained = harness.store.get_command(answer_id).await.unwrap().unwrap();
    assert_eq!(retained.receipt, answer.receipt);
    assert_eq!(retained.work.dispatch, delivered.work.dispatch);
    assert_eq!(
        harness.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        logs
    );
    semantic
}

#[tokio::test]
async fn public_approval_preserves_pending_identity_actor_and_one_answer_across_worker_replacement()
{
    let expected = exercise(Route::Agui).await;
    for route in [Route::Responses, Route::ResponsesV2] {
        assert_eq!(exercise(route).await, expected, "{route:?}");
    }
}
