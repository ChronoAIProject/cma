//! Internal lifecycle snapshots remain resolvable by workers, never by profile authoring.
use super::*;
use layer2_core::profiles::{
    AgentProfileStore, PrivateProfileCreationService, SandboxProfileStore, WorkspaceProfileStore,
};

#[tokio::test]
async fn internal_snapshots_are_hidden_before_paging_and_cannot_be_managed_by_id() {
    let store = Arc::new(InMemoryProfileStore::default());
    let router = fixture_with_profile_store(store.clone());
    let snapshots = PrivateProfileCreationService::new(
        store.clone(),
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 4_000,
            min_memory_mib: 256,
            max_memory_mib: 8_192,
        },
        Arc::new(["SECRET_TOKEN".into()]),
    );
    let owner = SubjectId::new("owner");
    let agent_body = agent_body("Ordinary agent");
    let agent = snapshots
        .create_agent_for_owner(
            &owner,
            "internal-agent",
            "Ordinary agent".into(),
            String::new(),
            serde_json::from_value(agent_body["content"].clone()).unwrap(),
        )
        .await
        .unwrap();
    let workspace_body = workspace_body("workspace", agent.profile_id.as_str(), 1);
    let workspace = snapshots
        .create_workspace_for_owner(
            &owner,
            "internal-workspace",
            "Ordinary workspace".into(),
            String::new(),
            serde_json::from_value(workspace_body["content"].clone()).unwrap(),
        )
        .await
        .unwrap();
    let sandbox_body = sandbox_body(
        "Ordinary sandbox",
        Some(workspace.profile_id.as_str()),
        vec![],
        None,
    );
    let sandbox = snapshots
        .create_sandbox_for_owner(
            &owner,
            "internal-sandbox",
            "Ordinary sandbox".into(),
            String::new(),
            serde_json::from_value(sandbox_body["content"].clone()).unwrap(),
        )
        .await
        .unwrap();
    assert!(
        store
            .agent_profile(&agent.profile_id)
            .await
            .unwrap()
            .unwrap()
            .internal
    );
    assert!(
        store
            .workspace_profile(&workspace.profile_id)
            .await
            .unwrap()
            .unwrap()
            .internal
    );
    assert!(
        store
            .sandbox_profile(&sandbox.profile_id)
            .await
            .unwrap()
            .unwrap()
            .internal
    );

    for (kind, internal_id, mut body) in [
        ("agent-profiles", agent.profile_id.as_str(), agent_body),
        (
            "workspace-profiles",
            workspace.profile_id.as_str(),
            workspace_body,
        ),
        (
            "sandbox-profiles",
            sandbox.profile_id.as_str(),
            sandbox_body,
        ),
    ] {
        // User-editable names and descriptions must never imply internal origin.
        body["name"] = json!("Inline Trigger user configuration");
        body["description"] = json!("Internal Agent Manager Trigger user description");
        let base = format!("/api/v1/{kind}");
        let mut visible = Vec::new();
        for index in 0..2 {
            let (status, created) = mutate(
                &router,
                Method::POST,
                &base,
                Some(body.clone()),
                "owner",
                &format!("{kind}-{index}"),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{created}");
            visible.push(created["id"].as_str().unwrap().to_owned());
            let (status, read) = json_response(
                &router,
                request(
                    Method::GET,
                    &format!("{base}/{}", visible.last().unwrap()),
                    None,
                    Some(("owner", true)),
                    None,
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(read["name"], body["name"]);
            assert!(read.get("internal").is_none());
        }
        let mut cursor = None;
        let mut listed = Vec::new();
        loop {
            let path = format!(
                "{base}?limit=1{}",
                cursor
                    .as_ref()
                    .map(|value| format!("&cursor={value}"))
                    .unwrap_or_default()
            );
            let (status, page) = json_response(
                &router,
                request(Method::GET, &path, None, Some(("owner", true)), None),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items = page["items"].as_array().unwrap();
            assert_eq!(
                items.len(),
                1,
                "internal rows must not consume a page: {page}"
            );
            listed.push(items[0]["id"].as_str().unwrap().to_owned());
            cursor = page["next"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
            assert!(listed.len() < 3, "pagination must terminate");
        }
        listed.sort();
        visible.sort();
        assert_eq!(listed, visible);
        for suffix in ["", "/revisions", "/revisions/1"] {
            assert_eq!(
                response(
                    &router,
                    request(
                        Method::GET,
                        &format!("{base}/{internal_id}{suffix}"),
                        None,
                        Some(("owner", true)),
                        None
                    )
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        body["draft_version"] = json!(1);
        for (method, suffix, payload) in [
            (Method::PUT, "", Some(body)),
            (Method::POST, "/revisions", Some(json!({"draft_version":1}))),
            (
                Method::PUT,
                "/visibility",
                Some(json!({"visibility":"public"})),
            ),
            (Method::POST, "/retire", None),
        ] {
            let (status, result) = mutate(
                &router,
                method,
                &format!("{base}/{internal_id}{suffix}"),
                payload,
                "owner",
                &format!("deny-{kind}-{suffix}"),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{result}");
        }
    }
    // Internal launch/provision workers still resolve the original immutable revisions.
    assert_eq!(
        store
            .agent_profile_revision(&agent.profile_id, 1)
            .await
            .unwrap(),
        Some(agent)
    );
    assert_eq!(
        store
            .workspace_profile_revision(&workspace.profile_id, 1)
            .await
            .unwrap(),
        Some(workspace)
    );
    assert_eq!(
        store
            .sandbox_profile_revision(&sandbox.profile_id, 1)
            .await
            .unwrap(),
        Some(sandbox)
    );
}
