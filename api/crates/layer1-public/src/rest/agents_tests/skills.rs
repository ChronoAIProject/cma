use super::*;
struct SupportedSkills;
#[async_trait]
impl crate::rest::AgentSkillRuntimeSupport for SupportedSkills {
    async fn supported(&self, _: &Sandbox) -> bool {
        true
    }
}

#[tokio::test]
async fn live_skills_routes_require_owner_key_and_supported_runtime_then_replay_pending() {
    let fixture = fixture().await;
    let agent = seeded_agent("agt_live_skills", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent.clone()).await.unwrap();
    let path = "/api/v1/agents/agt_live_skills/skills";
    let body = json!({"id":"research","version":"1.0"});
    let (status, before) = json_response(
        &fixture.router,
        request(Method::GET, path, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["can_install"], false);
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::POST,
            path,
            OWNER,
            Some("old-runtime"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(fixture
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .work
        .live_skills
        .is_empty());
    let router = agent_router(fixture.state.with_skill_support(Arc::new(SupportedSkills)));
    let (status, before) =
        json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["can_install"], true);
    let (status, _) = json_response(
        &router,
        request(Method::POST, path, OWNER, None, Some(body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = json_response(&router, request(Method::GET, path, OTHER, None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, first) = json_response(
        &router,
        request(
            Method::POST,
            path,
            OWNER,
            Some("skill-one"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["pending"], true);
    assert_eq!(first["can_install"], false);
    assert_eq!(first["items"][0]["status"], "pending");
    let (status, replay) = json_response(
        &router,
        request(Method::POST, path, OWNER, Some("skill-one"), Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(replay, first);
    let (status, read) =
        json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read, first);
    assert_eq!(
        fixture
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .live_skills
            .len(),
        1
    );
}

#[path = "skill_upgrade_tests.rs"]
mod upgrades;
