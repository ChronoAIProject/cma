use super::*;

struct SettingsRegistry {
    input: layer2_core::sandbox::AgentRuntimeInputSender,
}
#[async_trait]
impl AgentRuntimeRegistry for SettingsRegistry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: agent_capabilities(),
            input: self.input.clone(),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn settings_route_is_owner_only_requires_key_and_replays_exact_result() {
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = writes.clone();
    let input = layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, move |_| {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async { Ok(()) }
    });
    let fixture = fixture_with_registry(Arc::new(SettingsRegistry { input })).await;
    let agent = seeded_agent("agt_settings", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent).await.unwrap();
    let path = "/api/v1/agents/agt_settings/settings";
    let body = json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous"});
    let (status, _) = json_response(
        &fixture.router,
        request(Method::PATCH, path, OWNER, None, Some(body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OTHER,
            Some("other-settings"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, first) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("settings-one"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(
        first,
        json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous","pending":false})
    );
    let (status, replay) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("settings-one"),
            Some(body.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, first);
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (status, read) = json_response(
        &fixture.router,
        request(Method::GET, path, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read, first);
    let (status, _) = json_response(
        &fixture.router,
        request(Method::GET, path, OTHER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut different = body.clone();
    different["model"] = json!("other");
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("settings-one"),
            Some(different),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let mut invalid = body;
    invalid["model"] = json!("\n");
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::PATCH,
            path,
            OWNER,
            Some("invalid-settings"),
            Some(invalid),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
}
