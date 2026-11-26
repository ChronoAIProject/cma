use super::*;
use layer2_core::{
    profiles::{SkillRef, SkillSource},
    sandbox::{
        store::{AgentWorkMutation, AgentWorkerStore},
        AgentSkillInstallStatus, ResolvedAgentSkill,
    },
    SkillId,
};
struct Upgrades;
#[async_trait]
impl crate::rest::AgentSkillRuntimeSupport for Upgrades {
    async fn supported(&self, _: &Sandbox) -> bool {
        true
    }
    async fn upgrade_supported(&self, _: &Sandbox) -> bool {
        true
    }
}
async fn settle(store: &InMemorySandboxStore, id: &AgentId, version: &str, fail: bool) {
    let now = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
    let stamp = layer2_core::rfc3339_at(now);
    let mut current = store
        .claim_agent_work(
            id,
            "worker",
            stamp.clone(),
            layer2_core::rfc3339_at(now + std::time::Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    for pin in [true, false] {
        let mut next = current.clone();
        let mut logs = vec![];
        let item = next.work.live_skills.last_mut().unwrap();
        if pin {
            item.material = Some(ResolvedAgentSkill {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                name: Some("Research".into()),
                version: version.into(),
                archive_sha256: "a".repeat(64),
            });
        } else {
            item.status = if fail {
                AgentSkillInstallStatus::Failed
            } else {
                AgentSkillInstallStatus::Installed
            };
            item.error = fail.then(|| "runtime_skill_upgrade_conflict".into());
            logs.push(AgentLogKind::SkillInstallationChanged {
                skill_id: "research".into(),
                status: item.status,
            });
            next.work.schedule(AgentWorkStep::None, stamp.clone());
        }
        next.revision += 1;
        next.updated_at = stamp.clone();
        next.conversation.last_seq += logs.len() as u64;
        let mutation = AgentWorkMutation {
            agent: next.clone(),
            expected_agent_revision: current.revision,
            holder: "worker".into(),
            fence: current.work.lease.as_ref().unwrap().fence,
            worktree: None,
            appended_at: stamp.clone(),
            sandbox_event: None,
            agent_logs: logs,
        };
        if pin {
            store.commit_agent_work(mutation).await
        } else {
            let guard = skill_result_fixture::capture(store, &current).await;
            store.commit_agent_skill_result(mutation, guard).await
        }
        .unwrap();
        current = next;
    }
}
#[tokio::test]
async fn upgrade_routes_keep_one_effective_row_replay_keys_and_name_runtime_and_busy_refusals() {
    let mut fixture = fixture().await;
    fixture.state.agent_operations = fixture
        .state
        .agent_operations
        .with_skill_upgrades_enabled(true);
    let agent = seeded_agent("agt_upgrade", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent.clone()).await.unwrap();
    skill_result_fixture::prepare_parents(&fixture.store, &agent).await;
    let old = agent_router(
        fixture
            .state
            .clone()
            .with_skill_support(Arc::new(SupportedSkills)),
    );
    let router = agent_router(fixture.state.clone().with_skill_support(Arc::new(Upgrades)));
    let path = "/api/v1/agents/agt_upgrade/skills";
    let request_body = |version: &str| Some(json!({"id":"research", "version":version}));
    let (status, refused) = json_response(
        &fixture.router,
        request(
            Method::POST,
            path,
            OWNER,
            Some("unsupported-install"),
            request_body("1.0"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "agent_skill_install_unsupported");
    assert_eq!(
        json_response(
            &router,
            request(
                Method::POST,
                path,
                OWNER,
                Some("install"),
                request_body("1.0")
            )
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    settle(&fixture.store, &agent.id, "1.0", false).await;
    let original = fixture.store.get_agent(&agent.id).await.unwrap().unwrap();
    let (status, refused) = json_response(
        &old,
        request(
            Method::POST,
            path,
            OWNER,
            Some("unsupported-upgrade"),
            request_body("1.1"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "agent_skill_version_upgrade_unsupported");
    assert_eq!(
        json_response(
            &router,
            request(
                Method::POST,
                path,
                OWNER,
                Some("unsupported-upgrade"),
                request_body("1.1")
            )
        )
        .await
        .1,
        refused
    );
    let mut busy = original.clone();
    busy.status = AgentStatus::Working;
    busy.conversation.active_turn = Some(7);
    busy.revision += 1;
    fixture
        .store
        .save_agent(original.revision, busy.clone())
        .await
        .unwrap();
    let (status, refused) = json_response(
        &router,
        request(Method::POST, path, OWNER, Some("busy"), request_body("1.1")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["code"], "agent_not_ready");
    assert!(refused["detail"]
        .as_str()
        .unwrap()
        .contains("new Idempotency-Key"));
    let mut idle = busy.clone();
    idle.status = AgentStatus::Ready;
    idle.conversation.active_turn = None;
    idle.revision += 1;
    fixture.store.save_agent(busy.revision, idle).await.unwrap();
    assert_eq!(
        json_response(
            &router,
            request(Method::POST, path, OWNER, Some("busy"), request_body("1.1"))
        )
        .await
        .1,
        refused
    );
    let (status, first) = json_response(
        &router,
        request(
            Method::POST,
            path,
            OWNER,
            Some("upgrade"),
            request_body("1.1"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["pending"], true);
    assert_eq!(first["can_install"], false);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["version"], "1.0");
    assert_eq!(
        json_response(
            &router,
            request(
                Method::POST,
                path,
                OWNER,
                Some("upgrade"),
                request_body("1.1")
            )
        )
        .await
        .1,
        first
    );
    let (_, changed) = json_response(
        &router,
        request(
            Method::POST,
            path,
            OWNER,
            Some("upgrade"),
            request_body("1.2"),
        ),
    )
    .await;
    assert_eq!(changed["code"], "idempotency_key_reused");
    settle(&fixture.store, &agent.id, "1.1", false).await;
    let (_, read) = json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(read["items"].as_array().unwrap().len(), 1);
    assert_eq!(read["items"][0]["version"], "1.1");
    assert_eq!(read["items"][0]["status"], "installed");
    assert_eq!(read["can_install"], true);
    assert_eq!(
        json_response(
            &router,
            request(
                Method::POST,
                path,
                OWNER,
                Some("upgrade"),
                request_body("1.1")
            )
        )
        .await
        .1,
        first
    );
    assert_eq!(
        json_response(
            &router,
            request(
                Method::POST,
                path,
                OWNER,
                Some("failed-upgrade"),
                request_body("1.2")
            )
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    settle(&fixture.store, &agent.id, "1.2", true).await;
    let (_, read) = json_response(&router, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(read["items"].as_array().unwrap().len(), 1);
    assert_eq!(read["items"][0]["version"], "1.1");
    assert!(read["items"][0]["error"].as_str().unwrap().contains("1.2"));
    assert_eq!(read["pending"], false);
    let stored = fixture.store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.work.live_skills.len(), 3);
    assert_eq!(stored.work.live_skills[0], original.work.live_skills[0]);
}
#[test]
fn profile_overlay_has_one_row_and_keeps_the_installed_version_until_confirmation() {
    let mut agent = seeded_agent("agt_profile", AgentStatus::Ready, 0, None);
    let previous = ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: SkillId::new("research"),
        name: None,
        version: "1.0".into(),
        archive_sha256: "a".repeat(64),
    };
    agent.work.resolved_skills = Some(vec![previous.clone()]);
    let mut item = layer2_core::sandbox::AgentSkillInstallation::new(SkillRef {
        source: SkillSource::Ornn,
        id: previous.id.clone(),
        version: Some("1.1".into()),
    });
    item.previous = Some(previous);
    agent.work.live_skills.push(item);
    let view = crate::rest::agent_skills::AgentSkillsView::from(agent);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].version, "1.0");
    assert!(view.pending);
}

#[tokio::test]
async fn upgrade_gate_closure_preserves_reads_settlement_and_global_exact_replays() {
    let fixture = fixture().await;
    let agent = seeded_agent("agt_gate", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent.clone()).await.unwrap();
    skill_result_fixture::prepare_parents(&fixture.store, &agent).await;
    let closed = agent_router(fixture.state.clone().with_skill_support(Arc::new(Upgrades)));
    let mut state = fixture.state.clone().with_skill_support(Arc::new(Upgrades));
    state.agent_operations = state.agent_operations.with_skill_upgrades_enabled(true);
    let open = agent_router(state);
    let path = "/api/v1/agents/agt_gate/skills";
    let install = |key: &str, version: &str| {
        request(
            Method::POST,
            path,
            OWNER,
            Some(key),
            Some(json!({"id":"research", "version":version})),
        )
    };
    assert_eq!(
        json_response(&closed, install("add", "1.0")).await.0,
        StatusCode::ACCEPTED
    );
    settle(&fixture.store, &agent.id, "1.0", false).await;
    let refused = json_response(&closed, install("refused", "1.1")).await;
    assert_eq!(refused.0, StatusCode::CONFLICT);
    assert_eq!(refused.1["code"], "agent_skill_upgrade_disabled");
    assert_eq!(
        json_response(&open, install("refused", "1.1")).await,
        refused
    );
    let accepted = json_response(&open, install("accepted", "1.1")).await;
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    assert_eq!(
        json_response(&closed, install("accepted", "1.1")).await,
        accepted
    );
    settle(&fixture.store, &agent.id, "1.1", false).await;
    let read = json_response(&closed, request(Method::GET, path, OWNER, None, None)).await;
    assert_eq!(read.0, StatusCode::OK);
    assert_eq!(read.1["items"][0]["version"], "1.1");
    assert_eq!(
        json_response(&closed, install("accepted", "1.1")).await,
        accepted
    );
    assert_eq!(
        json_response(&closed, install("next", "1.2")).await.1["code"],
        "agent_skill_upgrade_disabled"
    );
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
        2
    );
}
