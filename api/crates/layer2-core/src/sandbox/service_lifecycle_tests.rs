use super::*;

#[tokio::test]
async fn lifecycle_admission_replays_after_lost_ack_and_keeps_trigger_namespace_separate() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let store = Arc::new(InMemorySandboxStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_lifecycle",
            "owner",
            "Research",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let credentials = Arc::new(FixedSandboxCredentialPrerequisite::Ready);
    let service = SandboxService::new(
        store.clone(),
        profiles.clone(),
        policy(10, 10),
        credentials.clone(),
    );
    let owner = caller(
        "owner",
        [Capability::UseOwnSandboxes, Capability::CreateSandbox],
    );
    let mut input = request(reference.clone());
    input.origin = SandboxOrigin::Lifecycle {
        operation_id: "operation-one".into(),
    };
    assert!(matches!(
        service.create(&owner, input.clone()).await,
        Err(SandboxServiceError::InvalidOrigin)
    ));
    let (left, right) = tokio::join!(
        service.create_for_lifecycle(&owner, input.clone()),
        service.create_for_lifecycle(&owner, input.clone())
    );
    let original = left.unwrap();
    assert_eq!(original.id, right.unwrap().id);

    // The operation may have crashed before recording the returned child ID.
    // Reopening with zero available quota still recovers the committed child.
    let reopened = SandboxService::new(store.clone(), profiles.clone(), policy(0, 0), credentials);
    assert_eq!(
        reopened
            .create_for_lifecycle(&owner, input.clone())
            .await
            .unwrap(),
        original
    );
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 1);
    assert_eq!(
        store
            .sandbox_events(&original.id, 0, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut conflicting = input.clone();
    conflicting.name = Some("Different".into());
    assert!(matches!(
        reopened.create_for_lifecycle(&owner, conflicting).await,
        Err(SandboxServiceError::SandboxOriginConflict)
    ));
    assert!(matches!(
        service
            .create_for_lifecycle(&caller("owner", [Capability::UseOwnSandboxes]), input)
            .await,
        Err(SandboxServiceError::Forbidden)
    ));

    let mut trigger = request(reference);
    trigger.origin = SandboxOrigin::Trigger {
        launch_id: "operation-one".into(),
    };
    let launched = service
        .create_for_launch(&owner, Capability::CreateSandbox, trigger)
        .await
        .unwrap();
    assert_ne!(launched.id, original.id);
    assert_eq!(
        store
            .list_sandboxes(&owner.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
}
