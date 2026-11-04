use super::*;

async fn resource_counts(database: &mongodb::Database) -> std::collections::BTreeMap<String, u64> {
    let mut counts = std::collections::BTreeMap::new();
    for name in database.list_collection_names().await.unwrap() {
        let count = database
            .collection::<mongodb::bson::Document>(&name)
            .count_documents(mongodb::bson::doc! {})
            .await
            .unwrap();
        counts.insert(name, count);
    }
    counts
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_recovery_rejects_changed_body_and_other_owner() {
    let (client, database, harness) = isolated_environment().await;
    let service = router(&harness, &database, &client).await;
    let before = resource_counts(&database).await;
    let availability: Value = serde_json::from_str(
        &cross_interface_tests::read(
            &service,
            &format!(
                "/api/v1/desktop-capability?workspace_id={}",
                harness.workspace.id
            ),
        )
        .await,
    )
    .unwrap();
    assert_eq!(availability["state"], "unsupported", "{availability}");
    let (refused, problem) = create(
        &service,
        &json!({"name":"Desktop task", "workspace_id":harness.workspace.id,
            "initial_message":"Open the page", "desktop":true}),
        "unverified-desktop",
    )
    .await;
    assert_eq!(refused, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "desktop_unsupported", "{problem}");
    assert_eq!(resource_counts(&database).await, before);
    let (accepted, receipt) = create(
        &service,
        &json!({"name":"Desktop task", "workspace_id":harness.workspace.id,
            "initial_message":"Open the page"}),
        "unverified-desktop",
    )
    .await;
    assert_eq!(accepted, StatusCode::ACCEPTED, "{receipt}");
    let input = json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":"  Exact task  "});
    let (status, receipt) = create(&service, &input, "owned-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let identity = admitted_identity(&service, &receipt).await;
    let mut changed = input.clone();
    changed["initial_message"] = json!("Exact task");
    let (status, problem) = create(&service, &changed, "owned-task").await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    let (_, replay) = create(&service, &input, "owned-task").await;
    assert_eq!(replay["id"], receipt["id"]);
    let foreign = router_for(&harness, &database, &client, "another-owner", false).await;
    for path in [
        format!(
            "/api/v1/agent-lifecycle/{}",
            receipt["id"].as_str().unwrap()
        ),
        format!("/api/v1/agents/{identity}"),
        format!("/api/v1/agents/{identity}/transcript"),
    ] {
        let response = foreign
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("authorization", TEST_BEARER)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let (status, denied) = create(&foreign, &input, "owned-task").await;
    assert!(
        matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
        "{status}: {denied}"
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_legacy_omission_keeps_automatic_setup_purpose() {
    let (client, database, harness) = isolated_environment().await;
    let service = router(&harness, &database, &client).await;
    let (status, receipt) = create(
        &service,
        &json!({"name":"Legacy agent", "workspace_id":harness.workspace.id}),
        "legacy-setup",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let identity = admitted_identity(&service, &receipt).await;
    let detail: Value = serde_json::from_str(
        &cross_interface_tests::read(&service, &format!("/api/v1/agents/{identity}")).await,
    )
    .unwrap();
    assert_eq!(detail["first_message"]["purpose"], "setup");
    database.drop().await.unwrap();
}
