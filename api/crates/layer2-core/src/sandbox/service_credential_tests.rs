use super::*;

struct Prerequisite(Mutex<Result<(), SandboxCredentialPrerequisiteError>>);

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for Prerequisite {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner.as_str(), "owner");
        self.0.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn missing_credential_prerequisite_admits_no_sandbox_or_profile_use() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_credentials",
            "owner",
            "Credentials",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let prerequisite = Arc::new(Prerequisite(Mutex::new(Err(
        SandboxCredentialPrerequisiteError::SetupRequired,
    ))));
    let service = SandboxService::new(
        store.clone(),
        profiles.clone(),
        policy(3, 1),
        prerequisite.clone(),
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    assert_eq!(
        service
            .create(&owner, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::CredentialSetupRequired
    );
    assert!(store
        .list_sandboxes(&owner.subject, &PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 0);
    let missing = vec![
        ServiceId::new("api-github"),
        ServiceId::new(AgentProvider::default().connection_services().subscription),
    ];
    *prerequisite.0.lock().unwrap() =
        Err(SandboxCredentialPrerequisiteError::ConnectionsRequired {
            services: missing.clone(),
        });
    assert_eq!(
        service
            .create(&owner, request(reference.clone()))
            .await
            .unwrap_err(),
        SandboxServiceError::ConnectionsRequired { services: missing }
    );
    assert!(store
        .list_sandboxes(&owner.subject, &PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 0);
    *prerequisite.0.lock().unwrap() = Ok(());
    assert_eq!(
        service
            .create(&owner, request(reference.clone()))
            .await
            .unwrap()
            .status,
        SandboxStatus::Provisioning
    );
    assert_eq!(sandbox_use_count(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn launch_sandbox_replay_precedes_a_changed_credential_prerequisite() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_replay_credentials",
            "owner",
            "Replay",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let store = Arc::new(InMemorySandboxStore::default());
    let prerequisite = Arc::new(Prerequisite(Mutex::new(Ok(()))));
    let service = SandboxService::new(store, profiles, policy(3, 1), prerequisite.clone());
    let owner = caller("owner", [Capability::LaunchTriggers]);
    let mut request = request(reference);
    request.origin = SandboxOrigin::Trigger {
        launch_id: "lch_credential_replay".into(),
    };
    let created = service
        .create_for_launch(&owner, Capability::LaunchTriggers, request.clone())
        .await
        .unwrap();
    *prerequisite.0.lock().unwrap() = Err(SandboxCredentialPrerequisiteError::SetupRequired);
    assert_eq!(
        service
            .create_for_launch(&owner, Capability::LaunchTriggers, request.clone())
            .await
            .unwrap(),
        created
    );
    request.origin = SandboxOrigin::Trigger {
        launch_id: "lch_new_credentials".into(),
    };
    assert_eq!(
        service
            .create_for_launch(&owner, Capability::LaunchTriggers, request)
            .await
            .unwrap_err(),
        SandboxServiceError::CredentialSetupRequired
    );
}

#[tokio::test]
async fn credential_dependency_failure_is_unavailable_after_existing_validation() {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let reference = publish(
        &profiles,
        profile(
            "sbp_unavailable_credentials",
            "owner",
            "Unavailable",
            ProfileVisibility::Private,
            false,
        ),
    )
    .await;
    let prerequisite = Arc::new(Prerequisite(Mutex::new(Err(
        SandboxCredentialPrerequisiteError::Unavailable("fixture unavailable".into()),
    ))));
    let service = SandboxService::new(
        Arc::new(InMemorySandboxStore::default()),
        profiles,
        policy(3, 1),
        prerequisite,
    );
    let owner = caller("owner", [Capability::CreateSandbox]);
    let mut invalid = request(reference.clone());
    invalid.name = Some(" ".into());
    assert_eq!(
        service.create(&owner, invalid).await.unwrap_err(),
        SandboxServiceError::InvalidName
    );
    assert!(matches!(
        service.create(&owner, request(reference)).await,
        Err(SandboxServiceError::Unavailable(_))
    ));
}
