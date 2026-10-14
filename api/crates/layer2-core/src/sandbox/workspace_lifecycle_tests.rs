use super::*;

#[tokio::test]
async fn lifecycle_workspace_replays_concurrent_admission_and_rejects_foreign_owner() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "wkp_lifecycle",
            "consumer",
            "empty-agent",
            ProfileVisibility::Private,
            vec![],
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = sandbox("sbx_lifecycle", SandboxStatus::Active, vec![], 4);
    store.create_sandbox(parent.clone()).await.unwrap();
    let service = WorkspaceService::new(store.clone(), profiles.clone());
    let owner = caller("consumer");
    let (first, second) = tokio::join!(
        service.create_for_lifecycle(&owner, None, &parent.id, &reference, "operation-one"),
        service.create_for_lifecycle(&owner, None, &parent.id, &reference, "operation-one")
    );
    let first = first.unwrap();
    assert_eq!(first.id, second.unwrap().id);
    assert!(matches!(first.source, WorkspaceSource::Empty));
    assert_eq!(
        first.origin,
        WorkspaceOrigin::Lifecycle {
            operation_id: "operation-one".into()
        }
    );
    let restarted = WorkspaceService::new(store.clone(), profiles.clone());
    assert_eq!(
        restarted
            .create_for_lifecycle(&owner, None, &parent.id, &reference, "operation-one")
            .await
            .unwrap()
            .id,
        first.id
    );
    assert!(matches!(
        restarted
            .create_for_lifecycle(
                &caller("foreign"),
                None,
                &parent.id,
                &reference,
                "operation-two"
            )
            .await,
        Err(WorkspaceServiceError::NotFound)
    ));
    let changed = publish(
        &profiles,
        profile(
            "wkp_different",
            "consumer",
            "different",
            ProfileVisibility::Private,
            vec![],
        ),
    )
    .await;
    assert_eq!(
        restarted
            .create_for_lifecycle(&owner, None, &parent.id, &changed, "operation-one")
            .await
            .unwrap_err(),
        WorkspaceServiceError::OriginConflict
    );
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
