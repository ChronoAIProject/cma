use super::*;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore};

#[tokio::test]
async fn partition_reset_http_requires_management_authority_and_replays_receipt() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-reset";
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(body()), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut binding = store.binding("binding-reset").await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    create_backing_resources(&store, &binding).await;
    store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    let mut workspace = store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, layer2_core::now_rfc3339());
    workspace.revision += 1;
    store.save_workspace(revision, workspace).await.unwrap();
    let mapping = BindingConversation::new(
        &binding,
        "oracle-partition".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    store
        .save_binding_conversation(binding.revision, None, mapping.clone())
        .await
        .unwrap();
    let mut agent =
        super::super::agents_tests::seeded_agent("agt_fixture", AgentStatus::Ready, 0, None);
    agent.id = mapping.agent_id.clone();
    agent.owner = binding.owner.clone();
    agent.sandbox_id = binding.sandbox_id.clone();
    agent.workspace_id = binding.workspace_id.clone();
    agent.worktree_id = mapping.worktree_id.clone();
    agent.status = AgentStatus::Deleted;
    agent.deleted_at = Some(layer2_core::now_rfc3339());
    agent.conversation.last_seq = 0;
    agent.conversation.active_turn = None;
    agent.work = AgentWorkState::new(AgentWorkStep::None, layer2_core::now_rfc3339());
    store.create_agent(agent).await.unwrap();
    let url = format!("{path}/conversations/oracle-partition/resets");
    let body = json!({"expected_agent_id":mapping.agent_id,"owner":binding.owner,"organization":false,"idempotency_binding_id":binding.binding_id,"request_digest":"a".repeat(64)});
    let key = "b".repeat(64);
    let make = |authority| {
        let mut req = request("POST", &url, Some(body.clone()), authority);
        req.headers_mut()
            .insert("idempotency-key", key.parse().unwrap());
        req
    };
    for (auth, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(Authorization::none()), StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            app.clone().oneshot(make(auth)).await.unwrap().status(),
            status
        );
    }
    let first = app.clone().oneshot(make(Some(authority()))).await.unwrap();
    let status = first.status();
    let first = json_body(first).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["incarnation"], 1);
    assert_eq!(first["conversation_id"], "oracle-partition");
    let second = app.clone().oneshot(make(Some(authority()))).await.unwrap();
    assert_eq!(json_body(second).await, first);
    let inspection = app
        .clone()
        .oneshot(request(
            "GET",
            &format!("{path}/conversations/oracle-partition"),
            None,
            Some(authority()),
        ))
        .await
        .unwrap();
    let inspection = json_body(inspection).await;
    assert_eq!(inspection["incarnation"], 1);
    assert_eq!(inspection["agent_id"], first["successor_agent_id"]);
}
