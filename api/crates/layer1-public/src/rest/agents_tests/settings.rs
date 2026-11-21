use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

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

#[tokio::test]
async fn settings_refusal_replays_after_dispatch_settles_and_requires_explicit_new_apply() {
    let writes = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let released = Arc::new(Notify::new());
    let input = layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, {
        let (writes, entered, released) = (writes.clone(), entered.clone(), released.clone());
        move |_| {
            let first_write = writes.fetch_add(1, Ordering::SeqCst) == 0;
            let (entered, released) = (entered.clone(), released.clone());
            async move {
                if first_write {
                    entered.notify_one();
                    released.notified().await;
                }
                Ok(())
            }
        }
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
    let mut competing = body.clone();
    competing["reasoning_effort"] = json!("medium");
    let router = fixture.router;
    let first = tokio::spawn({
        let router = router.clone();
        async move {
            response(
                &router,
                request(
                    Method::PATCH,
                    path,
                    OWNER,
                    Some("competing"),
                    Some(competing),
                ),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("competing update must enter dispatch");

    let apply = |key| request(Method::PATCH, path, OWNER, Some(key), Some(body.clone()));
    let (status, refused) = json_response(&router, apply("original")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "agent_operation_in_flight");
    assert_eq!(refused["settings_update_reason"], "dispatch_in_progress");
    assert!(refused.get("operation_sequence").is_none());
    released.notify_one();
    assert_eq!(first.await.unwrap().0, StatusCode::OK);

    let mut refused_access = apply("original");
    refused_access
        .extensions_mut()
        .insert(Authorization::new([]));
    let (status, denied) = json_response(&router, refused_access).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(denied.get("settings_update_reason").is_none());

    let (status, replay) = json_response(&router, apply("original")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(replay, refused);
    assert_eq!(writes.load(Ordering::SeqCst), 1);

    let (status, refreshed) =
        json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refreshed["reasoning_effort"], "medium");
    assert_eq!(refreshed["pending"], false);

    let (status, updated) = json_response(&router, apply("after-refresh")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["reasoning_effort"], "high");
    assert_eq!(
        json_response(&router, apply("after-refresh")).await,
        (status, updated)
    );
    assert_eq!(writes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn preferences_revision_reason_requires_an_explicit_revision_mismatch() {
    let fixture = fixture().await;
    let agent = seeded_agent("agt_preferences", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent).await.unwrap();
    let path = "/api/v1/agents/agt_preferences/preferences";
    let apply = |key, revision| {
        request(
            Method::PATCH,
            path,
            OWNER,
            Some(key),
            Some(json!({
                "expected_revision": revision, "title": "Updated title"
            })),
        )
    };
    let (status, rejected) = json_response(&fixture.router, apply("stale", 1)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["code"], "agent_operation_in_flight");
    assert_eq!(rejected["settings_update_reason"], "stale_revision");
    let (status, updated) = json_response(&fixture.router, apply("current", 0)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["title"], "Updated title");
    assert_eq!(updated["revision"], 1);
}
