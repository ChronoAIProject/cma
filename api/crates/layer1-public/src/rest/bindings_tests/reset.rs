use super::*;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore};

pub(in crate::rest) async fn fixture() -> (
    Router,
    Arc<InMemorySandboxStore>,
    ProviderBinding,
    BindingConversation,
) {
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
    let mut mapping = BindingConversation::new(
        &binding,
        "oracle-partition".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    mapping.agent_id = layer2_core::AgentId("agt_reset_predecessor".into());
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
    (app, store, binding, mapping)
}

#[tokio::test]
async fn partition_reset_http_requires_management_authority_and_replays_receipt() {
    let (app, _store, binding, mapping) = fixture().await;
    let path = "/api/v1/cmaeg/bindings/binding-reset";
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

#[tokio::test]
async fn source_user_transport_lifecycle_never_reserves_or_retires_an_agent() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-users";
    let mut input = body();
    input["profile"]["environment"]["session_affinity"] = json!("source_user");
    input["profile_digest"] = json!(format!(
        "sha256:{}",
        content_digest(&input["profile"]).unwrap()
    ));
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(input), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut binding = store.binding("binding-users").await.unwrap().unwrap();
    assert_eq!(
        binding.profile.session_affinity,
        SessionAffinity::SourceUser
    );
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    let generation = binding.generation;
    readiness::create_backing_resources(&store, &binding).await;
    store.save_binding(Some(revision), binding).await.unwrap();
    for transport in ["chat_one", "chat_two", "cma_user_untrusted_alias"] {
        let target = format!("{path}/conversations/{transport}");
        let response = app
            .clone()
            .oneshot(request("PUT", &target, None, Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await,
            json!({"conversation_id":transport,"state":"ready"})
        );
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(request("DELETE", &target, None, Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
    }
    assert!(store
        .binding_conversations("binding-users", generation, None, 100)
        .await
        .unwrap()
        .is_empty());
    for method in ["PUT", "DELETE"] {
        let response = app
            .clone()
            .oneshot(request(
                method,
                &format!("{path}/conversations/bad"),
                None,
                Some(authority()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .clone()
            .oneshot(request(
                method,
                &format!("{path}/conversations/chat_one"),
                None,
                Some(Authorization::none()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
