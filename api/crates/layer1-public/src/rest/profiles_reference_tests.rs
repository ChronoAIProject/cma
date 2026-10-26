use super::*;
use layer2_core::profiles::{
    AgentProfile, AgentProfileStore, ProfileVisibility, WorkspaceProfile, WorkspaceProfileStore,
};
use layer2_core::{
    AgentProfileId, WorkspaceProfileId, DEFAULT_AGENT_PROFILE_ID, DEFAULT_WORKSPACE_PROFILE_ID,
};

async fn create_and_publish(
    router: &Router,
    kind: &str,
    body: Value,
    owner: &str,
    key: &str,
) -> Value {
    let (status, profile) = mutate(
        router,
        Method::POST,
        &format!("/api/v1/{kind}"),
        Some(body),
        owner,
        key,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let path = format!(
        "/api/v1/{kind}/{}/revisions",
        profile["id"].as_str().unwrap()
    );
    let (status, result) = mutate(
        router,
        Method::POST,
        &path,
        Some(json!({"draft_version": 1})),
        owner,
        &format!("{key}-publish"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    profile
}

#[tokio::test]
async fn second_owner_forks_save_and_publish_with_private_and_retired_exact_pins() {
    let router = fixture(true);
    let agent = create_and_publish(
        &router,
        "agent-profiles",
        agent_body("Private child"),
        "publisher",
        "child-agent",
    )
    .await;
    let agent_id = agent["id"].as_str().unwrap();
    publish_agent(&router, "publisher", agent_id, 1, "agent-revision-two").await;
    let workspace = create_and_publish(
        &router,
        "workspace-profiles",
        workspace_body("source-workspace", agent_id, 1),
        "publisher",
        "source-workspace",
    )
    .await;
    let workspace_id = workspace["id"].as_str().unwrap();
    let sandbox = create_and_publish(
        &router,
        "sandbox-profiles",
        sandbox_body("Source sandbox", Some(workspace_id), vec![], None),
        "publisher",
        "source-sandbox",
    )
    .await;
    let sandbox_id = sandbox["id"].as_str().unwrap();
    assert_eq!(
        mutate(
            &router,
            Method::POST,
            &format!("/api/v1/workspace-profiles/{workspace_id}/revisions"),
            Some(json!({"draft_version": 1})),
            "publisher",
            "workspace-revision-two"
        )
        .await
        .0,
        StatusCode::CREATED
    );
    for (kind, source, child_kind, child, field) in [
        (
            "workspace-profiles",
            workspace_id,
            "agent-profiles",
            agent_id,
            "agent_profiles",
        ),
        (
            "sandbox-profiles",
            sandbox_id,
            "workspace-profiles",
            workspace_id,
            "workspace_profiles",
        ),
    ] {
        assert_eq!(
            set_visibility(
                &router,
                "publisher",
                kind,
                source,
                "public",
                &format!("public-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        let (status, fork) = mutate(
            &router,
            Method::POST,
            &format!("/api/v1/marketplace/{kind}/{source}/fork"),
            Some(json!({"revision": 1})),
            "consumer",
            &format!("fork-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{fork}");
        let fork_id = fork["id"].as_str().unwrap();
        let path = format!("/api/v1/{kind}/{fork_id}");
        let hidden_path = format!("/api/v1/{child_kind}/{child}/revisions/1");
        assert_eq!(
            json_response(
                &router,
                request(
                    Method::GET,
                    &hidden_path,
                    None,
                    Some(("consumer", true)),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let content = fork["draft"].clone();
        let update =
            json!({"draft_version": 1, "description": "Second owner edit", "content": content});
        let (status, saved) = mutate(
            &router,
            Method::PUT,
            &path,
            Some(update),
            "consumer",
            &format!("save-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("{path}/revisions"),
                Some(json!({"draft_version": 2})),
                "consumer",
                &format!("publish-{kind}")
            )
            .await
            .0,
            StatusCode::CREATED
        );
        assert_eq!(
            set_visibility(
                &router,
                "publisher",
                kind,
                source,
                "private",
                &format!("hide-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("/api/v1/{child_kind}/{child}/retire"),
                None,
                "publisher",
                &format!("retire-child-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        let update = json!({"draft_version": 2, "content": content});
        let (status, saved) = mutate(
            &router,
            Method::PUT,
            &path,
            Some(update),
            "consumer",
            &format!("save-retired-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("{path}/revisions"),
                Some(json!({"draft_version": 3})),
                "consumer",
                &format!("publish-retired-{kind}")
            )
            .await
            .0,
            StatusCode::CREATED
        );
        let mut replacement = content.clone();
        replacement[field][0]["revision"] = 2.into();
        let (status, problem) = mutate(
            &router,
            Method::PUT,
            &path,
            Some(json!({"draft_version": 3, "content": replacement})),
            "consumer",
            &format!("replace-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert_eq!(problem["code"], "profile_reference_invalid");
    }
}

#[tokio::test]
async fn reserved_defaults_are_accepted_only_as_exact_kind_matched_references() {
    let store = Arc::new(InMemoryProfileStore::default());
    let now = "2026-09-05T00:00:00Z";
    let agent = AgentProfile {
        internal: false,
        id: AgentProfileId(DEFAULT_AGENT_PROFILE_ID.into()),
        owner: SubjectId("system".into()),
        name: "Default agent".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: serde_json::from_value(agent_body("default")["content"].clone()).unwrap(),
        draft_version: 1,
        created_at: now.into(),
        updated_at: now.into(),
        retired_at: None,
    };
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, now.into())
        .await
        .unwrap();
    let workspace = WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(DEFAULT_WORKSPACE_PROFILE_ID.into()),
        owner: agent.owner,
        name: "Default workspace".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: serde_json::from_value(
            workspace_body("default", DEFAULT_AGENT_PROFILE_ID, 1)["content"].clone(),
        )
        .unwrap(),
        draft_version: 1,
        created_at: now.into(),
        updated_at: now.into(),
        retired_at: None,
    };
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, now.into())
        .await
        .unwrap();
    let router = fixture_with_profile_store(store);
    for (kind, body, field) in [
        (
            "workspace-profiles",
            workspace_body("defaults", DEFAULT_AGENT_PROFILE_ID, 1),
            "agent_profiles",
        ),
        (
            "sandbox-profiles",
            sandbox_body("Defaults", Some(DEFAULT_WORKSPACE_PROFILE_ID), vec![], None),
            "workspace_profiles",
        ),
    ] {
        let profile = create_and_publish(
            &router,
            kind,
            body.clone(),
            "consumer",
            &format!("create-default-{kind}"),
        )
        .await;
        let path = format!("/api/v1/{kind}/{}", profile["id"].as_str().unwrap());
        let (status, saved) = mutate(
            &router,
            Method::PUT,
            &path,
            Some(json!({"draft_version": 1, "content": body["content"]})),
            "consumer",
            &format!("save-default-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("{path}/revisions"),
                Some(json!({"draft_version": 2})),
                "consumer",
                &format!("publish-default-{kind}")
            )
            .await
            .0,
            StatusCode::CREATED
        );
        let wrong_kind = if field == "agent_profiles" {
            DEFAULT_WORKSPACE_PROFILE_ID
        } else {
            DEFAULT_AGENT_PROFILE_ID
        };
        for (index, invalid_id) in [
            wrong_kind,
            "agp_default_extra",
            "wkp_default_extra",
            "sbp_default",
            "default",
        ]
        .iter()
        .enumerate()
        {
            let mut invalid = body.clone();
            invalid["content"][field][0]["id"] = (*invalid_id).into();
            assert_eq!(
                mutate(
                    &router,
                    Method::POST,
                    &format!("/api/v1/{kind}"),
                    Some(invalid),
                    "consumer",
                    &format!("invalid-default-{kind}-{index}")
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    }
}
