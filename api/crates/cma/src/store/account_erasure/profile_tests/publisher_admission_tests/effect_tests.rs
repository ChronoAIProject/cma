use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_effect_refuses_committed_publisher_fence_with_unfenced_profiles() {
    let mut fixture = Fixture::new("publisher_sandbox_gate").await;
    fixture.executing(LaunchEffectKind::CreateSandbox).await;
    fixture.fence_publisher().await;
    // The service and its resource transaction begin after both fences commit.
    assert!(matches!(
        fixture.create_sandbox().await,
        Err(SandboxServiceError::Unavailable(_))
    ));
    assert_eq!(
        fixture
            .resources
            .sandboxes
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    fixture
        .seed
        .client
        .database(&fixture.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_effect_refuses_committed_publisher_fence_with_unfenced_profiles() {
    let mut fixture = Fixture::new("publisher_workspace_gate").await;
    let sandbox = fixture.ready_for_workspace().await;
    fixture.fence_publisher().await;
    assert!(matches!(
        fixture.create_workspace(&sandbox).await,
        Err(WorkspaceServiceError::Unavailable(_))
    ));
    assert_eq!(
        fixture
            .resources
            .workspaces
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    fixture
        .seed
        .client
        .database(&fixture.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_effects_succeed_and_committed_resources_replay_after_publisher_fence() {
    let mut fixture = Fixture::new("publisher_committed_replay").await;
    let sandbox = fixture.ready_for_workspace().await;
    let workspace = fixture.create_workspace(&sandbox).await.unwrap();
    fixture.fence_publisher().await;
    let mut retry = sandbox.clone();
    retry.id = SandboxId::generate();
    let admission = SandboxAdmission::new(
        retry,
        SandboxAdmissionName::Exact(NAME.into()),
        0,
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    assert!(
        matches!(fixture.resources.admit_catalogue_sandbox(admission).await.unwrap(),
        SandboxAdmissionResult::Existing(existing) if existing.id == sandbox.id)
    );
    let mut retry = workspace.clone();
    retry.id = layer2_core::WorkspaceId::generate();
    assert!(
        matches!(fixture.resources.admit_catalogue_workspace(WorkspaceAdmission {
        binding: None,
        workspace: retry,
        kind: WorkspaceAdmissionKind::Trigger,
        event_at: layer2_core::now_rfc3339(),
    }).await.unwrap(), WorkspaceAdmissionResult::Existing(existing) if existing.id == workspace.id)
    );
    assert_eq!(fixture.create_sandbox().await.unwrap().id, sandbox.id);
    assert_eq!(
        fixture.create_workspace(&sandbox).await.unwrap().id,
        workspace.id
    );
    fixture
        .seed
        .client
        .database(&fixture.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn direct_manual_catalogue_admission_refuses_canonically_retained_profile_pins() {
    let mut fixture = Fixture::new("publisher_manual_retained_refusal").await;
    fixture.executing(LaunchEffectKind::CreateSandbox).await;
    let mut sandbox = fixture.create_sandbox().await.unwrap();
    let db = fixture.seed.client.database(&fixture.seed.database);
    erase(&db, &fixture.seed.owner).await;
    sandbox.id = SandboxId::generate();
    sandbox.origin = SandboxOrigin::Manual;
    sandbox.name = "manual-retained-pin".into();
    let admission = SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Exact("manual-retained-pin".into()),
        10,
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    assert!(matches!(
        fixture.resources.admit_catalogue_sandbox(admission).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        fixture
            .resources
            .sandboxes
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
