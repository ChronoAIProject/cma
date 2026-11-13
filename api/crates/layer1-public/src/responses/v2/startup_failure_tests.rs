use super::*;
use layer2_core::sandbox::{
    store::{AgentLogStore, WorktreeStore},
    AgentLogKind, TypedFailure,
};

async fn failed_startup(f: &Fixture) {
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.status = AgentStatus::Starting;
    agent.ready_at = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent
        .fail(
            TypedFailure {
                class: "agent_runtime_unsupported".into(),
                detail: "channel tool session setup did not complete".into(),
            },
            layer2_core::now_rfc3339(),
        )
        .unwrap();
    f.store.save_agent(revision, agent).await.unwrap();
}

#[tokio::test]
async fn failed_open_replays_the_accepted_key_and_terminal_response() {
    let f = starting_fixture().await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response_id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    failed_startup(&f).await;
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    for alias in [false, true] {
        let replay = create(&f, alias).await;
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["x-cma-response-id"], response_id);
        let bytes = axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap();
        let frames = events(&bytes);
        assert_eq!(frames.last().unwrap()["type"], "response.failed");
        assert_eq!(
            frames.last().unwrap()["response"]["error"]["code"],
            "agent_unavailable"
        );
    }
    let stored = f
        .store
        .get_command(&record.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.status.is_terminal());
    assert!(stored.work.dispatch.is_none());
    assert!(f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn never_admitted_input_to_failed_open_has_a_stable_terminal_refusal() {
    let f = starting_fixture().await;
    failed_startup(&f).await;
    for alias in [false, true, false] {
        let response = create(&f, alias).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "agent_command_not_admissible");
    }
    assert!(f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

async fn starting_fixture() -> Fixture {
    let (state, store) = fixtures::conversation_fixture().await;
    let mut agent = fixtures::seeded_agent("agt_fixture", AgentStatus::Starting, 1, None);
    agent.ready_at = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    store
        .create_worktree(fixtures::seeded_worktree(&agent))
        .await
        .unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            agent.created_at.clone(),
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        )
        .await
        .unwrap();
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    Fixture {
        router: super::super::router(super::super::ResponsesV2State::new(
            crate::agent_page::AgentPageOrigin::test(),
            commands,
            state.reads.with_resource_access(access.clone()),
        )),
        store,
        access,
    }
}

#[tokio::test]
async fn revival_refusals_keep_legacy_status_and_response_codes() {
    for failure in [
        CommandFailureCode::CredentialSetupRequired,
        CommandFailureCode::ConnectionsRequired,
    ] {
        let f = fixture().await;
        let response = create(&f, false).await;
        drop(response);
        let record = f
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0);
        f.store
            .reject_unclaimed_command(
                &record.receipt.envelope.command_id,
                record.revision,
                failure,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let replay = create(&f, false).await;
        let bytes = axum::body::to_bytes(replay.into_body(), usize::MAX)
            .await
            .unwrap();
        let frames = events(&bytes);
        let terminal = frames.last().unwrap();
        assert_eq!(terminal["type"], "response.failed");
        assert_eq!(
            terminal["response"]["error"]["code"],
            "agent_command_execution_failed"
        );
        assert_eq!(
            terminal["response"]["error"]["message"],
            "The admitted Agent command could not complete."
        );
        let request = request(
            Method::GET,
            &format!("/api/v2/commands/{}", record.receipt.envelope.command_id),
            "owner-a",
            None,
            None,
        );
        let (status, body) = fixtures::json_response(&f.router, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "failed");
        assert_eq!(body["failure"], "execution_failed");
        assert!(body["delivery"].is_null());
    }
}
