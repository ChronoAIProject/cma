use super::*;

fn placement(shared_workspace: bool) -> serde_json::Value {
    let mut value = json!({"kind":"workspace_per_partition","sandbox_id":"sandbox-reused",
        "workspace_profile":{"id":"workspace-profile","revision":2},"agent_profile":{"id":"agent-profile","revision":3},
        "shared_trust":"all_participants_share_filesystem_processes_and_credentials",
        "capacity":{"keep_other_workspaces":1,"keep_other_agents_per_workspace":2}});
    if shared_workspace {
        value["kind"] = "worktree_agent_per_partition".into();
        value["workspace_id"] = "workspace-reused".into();
        value.as_object_mut().unwrap().remove("workspace_profile");
    }
    value
}

#[tokio::test]
async fn api_persists_both_shared_placement_contracts_without_parent_creation_or_implicit_trust() {
    for shared_workspace in [false, true] {
        let (router, store, _) = fixture();
        let mut body = create_body(false);
        body["config"]["placement"] = placement(shared_workspace);
        let (status, receipt) = send(
            &router,
            Method::POST,
            "/api/v1/bridges",
            "owner",
            Some("reuse-create"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{receipt}");
        let id = receipt["bridge_id"].as_str().unwrap();
        let row = store.bridge(&BridgeId(id.into())).await.unwrap().unwrap();
        let config = &row.current().unwrap().config;
        assert!(config.placement.reused_parent().is_some());
        let (status, detail) = send(
            &router,
            Method::GET,
            &format!("/api/v1/bridges/{id}/configuration"),
            "owner",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["config"]["placement"], body["config"]["placement"]);
        for change in ["trust", "zero_capacity", "profile"] {
            let mut invalid = body.clone();
            match change {
                "trust" => {
                    invalid["config"]["placement"]
                        .as_object_mut()
                        .unwrap()
                        .remove("shared_trust");
                }
                "zero_capacity" => {
                    invalid["config"]["placement"]["capacity"]["keep_other_workspaces"] = 0.into()
                }
                _ => invalid["config"]["placement"]["agent_profile"]["revision"] = 0.into(),
            }
            let (status, _) = send(
                &router,
                Method::POST,
                "/api/v1/bridges",
                "owner",
                Some(change),
                Some(invalid),
            )
            .await;
            assert!(status.is_client_error(), "{change}: {status}");
        }
    }
}

#[tokio::test]
async fn incompatible_parent_has_precise_public_problem() {
    use axum::response::IntoResponse;
    let response = super::super::bridges::problem::BridgeProblem(
        layer2_core::bridge::BridgeError::ConnectionRequired(
            layer2_core::bridge::BridgeSetupBlock::IncompatibleTarget,
        ),
    )
    .into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(serde_json::to_string(&json)
        .unwrap()
        .contains("bridge_incompatible_target"));
}
