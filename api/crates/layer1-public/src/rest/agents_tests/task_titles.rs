use super::*;
use layer2_core::sandbox::{
    AgentMessageAssembly, AgentRuntimeInputSender, AgentRuntimeUpdate, AgentUpdateRecorder,
    DurableAgentUpdateRecorder,
};
use layer2_core::Caller;

struct TitleRuntime;

#[async_trait]
impl AgentRuntimeRegistry for TitleRuntime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: agent_capabilities(),
            input: AgentRuntimeInputSender::spawn(4, |_| async { Ok(()) }),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

fn owner() -> Caller {
    Caller::new(
        SubjectId::new(OWNER),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

async fn read_title(fixture: &Fixture, path: &str) -> Value {
    let router = agent_router(fixture.state.clone());
    let (status, body) =
        json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn task_titles_persist_first_task_fallback_before_provider_output_and_reopen() {
    let fixture = fixture_with_registry(Arc::new(TitleRuntime)).await;
    let agent = seeded_agent("agt_task_title", AgentStatus::Ready, 2, None);
    insert_agent(&fixture.store, agent.clone(), vec![]).await;
    fixture
        .state
        .agent_operations
        .send_message(&owner(), &agent.id, "比较 Rust 和 Go 的部署工具".into())
        .await
        .unwrap();
    let detail = read_title(&fixture, "/api/v1/agents/agt_task_title").await;
    assert_eq!(
        detail["runtime_metadata"]["description"],
        "比较 Rust 和 Go 的部署工具"
    );
    assert_eq!(
        detail["runtime_metadata"]["description_source"],
        "prompt_preview"
    );
    assert_eq!(
        detail["runtime_metadata"]["description_is_setup_preview"],
        false
    );
    let list = read_title(&fixture, "/api/v1/agents").await;
    assert_eq!(
        list["items"][0]["runtime_metadata"],
        detail["runtime_metadata"]
    );
    let events = read_title(&fixture, "/api/v1/agents/agt_task_title/events").await;
    assert!(
        events.to_string().contains("runtime_metadata_changed"),
        "{events}"
    );
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::GET,
            "/api/v1/agents/agt_task_title",
            OTHER,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn observe(
    fixture: &Fixture,
    agent: &Agent,
    title: &str,
    source: Option<layer2_core::sandbox::AgentDescriptionSource>,
) {
    let mut update = AgentRuntimeUpdate::Metadata(Default::default());
    if let AgentRuntimeUpdate::Metadata(metadata) = &mut update {
        metadata.description = Some(title.into());
        metadata.description_source = source;
    }
    DurableAgentUpdateRecorder::new(fixture.store.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn task_titles_keep_real_task_on_reconnect_and_owner_override_after_late_provider_updates() {
    use layer2_core::sandbox::AgentDescriptionSource::{PromptPreview, ProviderTitle};
    let fixture = fixture_with_registry(Arc::new(TitleRuntime)).await;
    let agent = seeded_agent("agt_title_race", AgentStatus::Ready, 2, None);
    insert_agent(&fixture.store, agent.clone(), vec![]).await;
    fixture
        .state
        .agent_operations
        .send_message(&owner(), &agent.id, "比较 Rust 和 Go 的部署工具".into())
        .await
        .unwrap();
    for source in [Some(PromptPreview), None] {
        observe(&fixture, &agent, "Prepare the workspace and wait", source).await;
        let detail = read_title(&fixture, "/api/v1/agents/agt_title_race").await;
        assert_eq!(
            detail["runtime_metadata"]["description"],
            "比较 Rust 和 Go 的部署工具"
        );
    }
    observe(&fixture, &agent, "Tooling comparison", Some(ProviderTitle)).await;
    let path = "/api/v1/agents/agt_title_race/preferences";
    let (status, result) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("rename-title"),
            Some(json!({"title":"工具选型", "expected_revision":0})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    observe(&fixture, &agent, "Old provider title", Some(ProviderTitle)).await;
    let detail = read_title(&fixture, "/api/v1/agents/agt_title_race").await;
    assert_eq!(detail["owner_title"], "工具选型");
    assert_eq!(
        read_title(&fixture, "/api/v1/agents").await["items"][0]["owner_title"],
        "工具选型"
    );
    let (status, result) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("rename-title-again"),
            Some(json!({"title":"工具选型定稿", "expected_revision":1})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        read_title(&fixture, "/api/v1/agents/agt_title_race").await["owner_title"],
        "工具选型定稿"
    );
}

#[tokio::test]
async fn task_titles_initial_task_uses_explicit_purpose_and_never_names_setup() {
    use layer2_core::sandbox::{
        AgentWorkAuthority, AgentWorkLease, FirstMessagePurpose, FirstMessageRecord,
    };
    for purpose in [FirstMessagePurpose::Task, FirstMessagePurpose::Setup] {
        let fixture = fixture_with_registry(Arc::new(TitleRuntime)).await;
        let mut agent = seeded_agent("agt_initial_title", AgentStatus::Ready, 2, None);
        let mut first =
            FirstMessageRecord::new("initial-task", "Prepare the release report", NOW).unwrap();
        first.purpose = Some(purpose);
        agent.conversation.first_message = Some(first);
        agent.work.step = AgentWorkStep::DispatchFirstMessage;
        agent.work.next_fence = 2;
        agent.work.lease = Some(AgentWorkLease {
            holder: "title-worker".into(),
            fence: 1,
            expires_at: "2026-11-04T00:00:00Z".into(),
        });
        insert_agent(&fixture.store, agent.clone(), vec![]).await;
        fixture
            .state
            .agent_operations
            .dispatch_queued_first_message(
                &agent.id,
                AgentWorkAuthority::new("title-worker".into(), 1).unwrap(),
            )
            .await
            .unwrap();
        let detail = read_title(&fixture, "/api/v1/agents/agt_initial_title").await;
        if purpose == FirstMessagePurpose::Task {
            assert_eq!(
                detail["runtime_metadata"]["description"],
                "Prepare the release report"
            );
            assert_eq!(
                detail["runtime_metadata"]["description_source"],
                "prompt_preview"
            );
        } else {
            assert!(detail.get("runtime_metadata").is_none(), "{detail}");
        }
    }
}

#[tokio::test]
async fn task_titles_queued_task_retains_a_bounded_preview_without_waiting_for_runtime() {
    let fixture = fixture_with_registry(Arc::new(TitleRuntime)).await;
    let agent = seeded_agent("agt_queued_title", AgentStatus::Paused, 2, None);
    insert_agent(&fixture.store, agent.clone(), vec![]).await;
    fixture.state.agent_operations.send_message(&owner(), &agent.id,
        "Review the release checklist for the deployment pipeline and produce a concise risk report for the team".into(),
    ).await.unwrap();
    let detail = read_title(&fixture, "/api/v1/agents/agt_queued_title").await;
    assert_eq!(
        detail["runtime_metadata"]["description"],
        "Review the release checklist for the deployment"
    );
    assert_eq!(
        detail["runtime_metadata"]["description_source"],
        "prompt_preview"
    );
}
