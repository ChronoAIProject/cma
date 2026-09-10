use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{AgentOrigin, AgentReadService, AgentStatus};
use layer2_core::trigger_domain::TriggerStoreError;
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use serde_json::json;
use std::sync::Arc;

use super::support_tests::*;
use crate::rest::agents_tests as agents;

#[tokio::test]
async fn prepared_profile_agent_keeps_origin_and_exposes_its_adopting_trigger() {
    use layer2_core::trigger_domain::{
        LaunchCommit, LaunchResolution, LaunchStore, ResolvedResource,
    };
    use layer2_core::LaunchId;
    let app = TestApp::new().await;
    let fixture = agents::fixture().await;
    let trigger = create_trigger(&app, "always_new", "prepared-source").await;
    let public = publish_trigger(&app, trigger["id"].as_str().unwrap(), "prepared-publish").await;
    let path = format!(
        "/api/v1/triggers/public/{}/launches",
        public["public_id"].as_str().unwrap()
    );
    let (status, created) = json_response(
        &app.router,
        request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some(("owner-a", visitor_authority())),
            Some("prepared-launch"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut agent = agents::seeded_agent("agt_prepared_source", AgentStatus::Ready, 2, None);
    agent.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    agent.work.warm_conversation_id = agent.conversation.conversation_id.clone();
    let agent = agents::insert_agent(&fixture.store, agent, vec![]).await;
    let mut launch = app
        .launches
        .launch(
            &LaunchId(created["id"].as_str().unwrap().into()),
            &agent.owner,
        )
        .await
        .unwrap()
        .unwrap();
    let expected_revision = launch.revision;
    launch.revision += 1;
    launch.resolution = LaunchResolution {
        sandbox: ResolvedResource::Reused(agent.sandbox_id.clone()),
        workspace: ResolvedResource::Reused(agent.workspace_id.clone()),
        agent: ResolvedResource::Reused(agent.id.clone()),
        choice_required: false,
    };
    app.launches
        .commit_launch(LaunchCommit {
            expected_revision,
            event_at: launch.updated_at.clone(),
            launch,
            events: vec![],
        })
        .await
        .unwrap();
    let reads = AgentReadService::new(fixture.store, Arc::new(agents::TestResources));
    let router = crate::rest::agent_source_router(reads.clone(), app.source_service.clone());
    let (status, source) = json_response(
        &router,
        agents::request(
            Method::GET,
            "/api/v1/agents/agt_prepared_source/source",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{source}");
    assert_eq!(source["origin"]["type"], "workspace_profile");
    assert_eq!(source["trigger"]["public_id"], public["public_id"]);
    assert_eq!(
        source["trigger"]["title"],
        public["content"]["presentation"]["title"]
    );
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert_eq!(reads.read(&caller, &agent.id).await.unwrap(), agent);
}

#[tokio::test]
async fn source_retains_provenance_and_uses_only_published_shortcuts_without_mutating_agent() {
    let app = TestApp::new().await;
    let fixture = agents::fixture().await;
    let trigger = create_trigger(&app, "always_new", "create-source").await;
    let id = trigger["id"].as_str().unwrap();
    let published = publish_trigger(&app, id, "publish-source").await;
    let path = format!(
        "/api/v1/triggers/public/{}/launches",
        published["public_id"].as_str().unwrap()
    );
    let (status, launch) = json_response(
        &app.router,
        request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some(("owner-a", visitor_authority())),
            Some("source-launch"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{launch}");
    let mut agent = agents::seeded_agent("agt_source", AgentStatus::Ready, 2, None);
    agent.origin = AgentOrigin::Trigger {
        trigger_id: id.into(),
        launch_id: launch["id"].as_str().unwrap().into(),
    };
    let agent = agents::insert_agent(&fixture.store, agent, vec![]).await;
    let before = agent.clone();
    let reads = AgentReadService::new(fixture.store, Arc::new(agents::TestResources));
    let router = fixture.router.merge(crate::rest::agent_source_router(
        reads.clone(),
        app.source_service.clone(),
    ));
    let source_path = "/api/v1/agents/agt_source/source";
    let (status, source) = json_response(
        &router,
        agents::request(Method::GET, source_path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{source}");
    assert_eq!(source["origin"]["type"], "trigger");
    assert_eq!(source["trigger"]["quick_messages"], json!([]));

    let messages = json!([{ "name":"Introduce the project", "description":"Explain this repository.", "text":"Read the introduction and explain this project." }]);
    let mut content = trigger["draft"].clone();
    content["quick_messages"] = messages.clone();
    let (status, _) = json_response(
        &app.router,
        publisher_request(
            Method::PUT,
            &format!("/api/v1/triggers/{id}"),
            Some(json!({"draft_version":1,"content":content})),
            Some("save-shortcuts"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, source) = json_response(
        &router,
        agents::request(Method::GET, source_path, "owner-a", None, None),
    )
    .await;
    assert_eq!(source["trigger"]["quick_messages"], json!([]));
    let (status, _) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/publish"),
            Some(json!({"draft_version":2})),
            Some("publish-shortcuts"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, source) = json_response(
        &router,
        agents::request(Method::GET, source_path, "owner-a", None, None),
    )
    .await;
    assert_eq!(source["trigger"]["quick_messages"], messages);
    assert_eq!(source["trigger"]["revision"], 2);
    assert_eq!(source["origin"]["launch_id"], launch["id"]);

    let (status, _) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/retire"),
            None,
            Some("retire-shortcuts"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, source) = json_response(
        &router,
        agents::request(Method::GET, source_path, "owner-a", None, None),
    )
    .await;
    assert_eq!(source["trigger"]["quick_messages"], json!([]));
    assert_eq!(
        source["trigger"]["title"],
        published["content"]["presentation"]["title"]
    );

    app.definitions.fail(
        "trigger_by_public_id",
        TriggerStoreError::Unavailable("private dependency detail".into()),
    );
    let (status, body) = json_response(
        &router,
        agents::request(Method::GET, source_path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("private dependency detail"));
    let owner = Caller::new(
        SubjectId::new("owner-a"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert_eq!(reads.read(&owner, &agent.id).await.unwrap(), before);
}

#[tokio::test]
async fn source_requires_owner_and_capability_and_manual_origins_do_not_read_trigger_dependencies()
{
    let app = TestApp::new().await;
    let fixture = agents::fixture().await;
    let agent = agents::insert_agent(
        &fixture.store,
        agents::seeded_agent("agt_manual", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let reads = AgentReadService::new(fixture.store, Arc::new(agents::TestResources));
    let router = crate::rest::agent_source_router(reads, app.source_service.clone());
    app.definitions
        .fail("*", TriggerStoreError::Unavailable("offline".into()));
    let path = "/api/v1/agents/agt_manual/source";
    let (status, source) = json_response(
        &router,
        agents::request(Method::GET, path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        source,
        json!({"agent_id":"agt_manual","origin":{"type":"manual"}})
    );
    let foreign = raw_response(
        &router,
        agents::request(Method::GET, path, "owner-b", None, None),
    )
    .await;
    let absent = raw_response(
        &router,
        agents::request(
            Method::GET,
            "/api/v1/agents/agt_missing/source",
            "owner-b",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(foreign.0, StatusCode::NOT_FOUND);
    assert_eq!(foreign.2, absent.2);
    let (status, _) = json_response(
        &router,
        request(
            Method::GET,
            path,
            None,
            Some(("owner-a", Authorization::none())),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = json_response(&router, request(Method::GET, path, None, None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let owner = Caller::new(
        SubjectId::new("owner-a"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut profile_agent = agent;
    profile_agent.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    assert!(app
        .source_service
        .agent_source(&owner, &profile_agent)
        .await
        .unwrap()
        .is_none());
}
