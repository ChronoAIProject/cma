use super::{
    binding_tests::{fixture, request},
    event_context_tests::context,
};
use crate::rest::agents_tests as fixtures;
use axum::{body::to_bytes, http::StatusCode};
use layer2_core::{
    profiles::content_digest,
    sandbox::{
        agent_io::{external_event_key, AgentCommandStore},
        binding::*,
        store::{AgentStore, WorkspaceStore, WorktreeStore},
        AgentStatus, AgentStopReason, AgentWorkState, AgentWorkStep,
    },
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn reset_routes_queued_event_to_successor_and_refuses_predecessor_replay() {
    let (app, store, _, principal) = fixture().await;
    let event = context();
    let body = json!({"conversation":"conversation-external","stream":true,"input":"hello","event_context":event});
    let mut forged = body.clone();
    forged["event_context"]["conversation_id"] = "unrelated-conversation".into();
    let refused = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, forged))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.headers().get("x-cma-response-id").is_none());
    let first = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let original = store
        .get_external_event_command(&external_event_key(
            "binding:binding-responses",
            "conversation-external",
            "event-1",
        ))
        .await
        .unwrap()
        .unwrap();
    let binding = store.binding("binding-responses").await.unwrap().unwrap();
    let previous = store
        .binding_conversation(
            &binding.binding_id,
            binding.generation,
            "conversation-external",
        )
        .await
        .unwrap()
        .unwrap();
    let mut agent = store.get_agent(&previous.agent_id).await.unwrap().unwrap();
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        let rev = agent.revision;
        agent
            .transition(
                status,
                layer2_core::now_rfc3339(),
                (status == AgentStatus::Stopped).then_some(AgentStopReason::Owner),
            )
            .unwrap();
        agent.work = AgentWorkState::new(AgentWorkStep::None, layer2_core::now_rfc3339());
        store.save_agent(rev, agent.clone()).await.unwrap();
    }
    let mut workspace = store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.work = layer2_core::sandbox::WorkspaceWorkState::new(
        layer2_core::sandbox::WorkspaceWorkStep::None,
        layer2_core::now_rfc3339(),
    );
    workspace.revision += 1;
    store.save_workspace(revision, workspace).await.unwrap();
    let receipt = store
        .reset_binding_conversation(
            ConversationResetRequest {
                binding_id: binding.binding_id.clone(),
                idempotency_binding_id: binding.binding_id.clone(),
                conversation_id: "conversation-external".into(),
                expected_agent_id: agent.id.clone(),
                owner: binding.resource_owner(),
                manager: binding.manager.clone(),
                operation_id: content_digest(&"reset").unwrap(),
                request_digest: content_digest(&"body").unwrap(),
            },
            true,
        )
        .await
        .unwrap();
    let current = store.binding(&binding.binding_id).await.unwrap().unwrap();
    let mut successor = receipt.head.successor;
    store
        .save_binding_conversation(current.revision, None, successor.clone())
        .await
        .unwrap();
    let mut new_agent =
        fixtures::seeded_agent(successor.agent_id.as_str(), AgentStatus::Ready, 0, None);
    new_agent.worktree_id = successor.worktree_id.clone();
    new_agent.creation_key = successor.creation_key().unwrap();
    store
        .create_worktree(fixtures::seeded_worktree(&new_agent))
        .await
        .unwrap();
    store.create_agent(new_agent.clone()).await.unwrap();
    successor.state = BindingConversationState::Ready;
    successor.revision += 1;
    store
        .save_binding_conversation(current.revision, Some(1), successor.clone())
        .await
        .unwrap();
    let replay = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, body.clone()))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    assert!(replay.headers().get("x-cma-response-id").is_none());
    let problem: Value =
        serde_json::from_slice(&to_bytes(replay.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(problem["error"]["code"], "agent_command_key_conflict");
    let mut queued = body;
    queued["event_context"]["activity"]["event_id"] = "queued-before-reset".into();
    let response = app
        .clone()
        .oneshot(request("/api/v2/responses", &principal, queued.clone()))
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(status, StatusCode::OK);
    let repeated = app
        .oneshot(request("/api/v2/responses", &principal, queued))
        .await
        .unwrap();
    assert_eq!(
        response.headers().get("x-cma-response-id"),
        repeated.headers().get("x-cma-response-id")
    );
    let new = store
        .get_external_event_command(&external_event_key(
            "binding:binding-responses",
            "conversation-external",
            "queued-before-reset",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new.receipt.envelope.agent_id, new_agent.id);
    assert_eq!(
        new.receipt
            .envelope
            .command
            .external_event()
            .unwrap()
            .provider_conversation_id
            .as_deref(),
        Some(successor.conversation_id.as_str())
    );
    let retained = store
        .get_external_event_command(&external_event_key(
            "binding:binding-responses",
            "conversation-external",
            "event-1",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, original);
}
