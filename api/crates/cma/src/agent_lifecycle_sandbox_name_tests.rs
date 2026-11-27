use super::{create_sandbox_with_fence, sandbox_name};
use crate::agent_lifecycle::validation::{sandbox_error, validate_request};
use crate::agent_lifecycle::*;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{
    CreateSandbox, FixedSandboxCredentialPrerequisite, SandboxLimits, SandboxOrigin, SandboxPolicy,
};

async fn admission(
    profiles: &PrivateProfileCreationService,
    caller: &Caller,
    title: &str,
    operation_id: &str,
) -> CreateSandbox {
    // The minimal portal request supplies only the visible Agent title. Reload
    // the serialized request as a reclaimed lifecycle worker would do.
    let request: AgentLifecycleCreateRequest =
        serde_json::from_value(json!({"name": title})).unwrap();
    validate_request(&request).unwrap();
    let request = serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
    let (_, _, profile) = profiles::create_profiles(
        profiles,
        caller,
        &request,
        None,
        operation_id,
        layer3_adapter::SandboxCapacity::default(),
        SandboxLimits {
            max_workspaces: 16,
            max_agents_per_workspace: 16,
        },
        LifecycleBehavior::PromptRepository,
        Default::default(),
        "",
    )
    .await
    .unwrap();
    CreateSandbox {
        profile,
        name: Some(sandbox_name(&request.name, operation_id)),
        confirmed_digest: None,
        origin: SandboxOrigin::Lifecycle {
            operation_id: operation_id.into(),
        },
    }
}

fn profiles(store: Arc<InMemoryProfileStore>) -> Arc<PrivateProfileCreationService> {
    PrivateProfileCreationService::new(
        store,
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        Arc::new([] as [String; 0]),
    )
}

fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]),
    )
}

fn lease(operation_id: &str) -> LifecycleLease {
    LifecycleLease {
        id: operation_id.to_owned(),
        token: "fixture-claim".into(),
    }
}

#[tokio::test]
async fn full_sandbox_quota_is_a_typed_terminal_failure_without_another_admission() {
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let profiles = profiles(profile_store.clone());
    let store = Arc::new(InMemorySandboxStore::default());
    let caller = owner();
    let service = SandboxService::new(
        store.clone(),
        profile_store,
        SandboxPolicy {
            max_sandboxes_per_owner: 1,
            ..SandboxPolicy::default()
        },
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let first = admission(&profiles, &caller, "First", "quota-first").await;
    let original = service.create_for_lifecycle(&caller, first).await.unwrap();
    let second = admission(&profiles, &caller, "Second", "quota-second").await;
    let error = create_sandbox(
        &service,
        &caller,
        second.profile,
        "Second",
        &lease("quota-second"),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        SandboxServiceError::SandboxLimitReached {
            limit: 1,
            live: 1,
            usage: Default::default()
        }
    );
    // Only Unavailable is automatically retried by the lifecycle worker.
    assert!(matches!(
        sandbox_error(error),
        AgentLifecycleProvisioningError::SandboxLimitReached {
            limit: 1,
            live: 1,
            ..
        }
    ));
    assert_eq!(
        store
            .list_sandboxes(&caller.subject, &Default::default())
            .await
            .unwrap()
            .items,
        vec![original]
    );
}

#[tokio::test]
async fn repeated_minimal_agent_creation_has_distinct_sandboxes_and_replays_after_restart() {
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let profiles = profiles(profile_store.clone());
    let store = Arc::new(InMemorySandboxStore::default());
    let caller = owner();
    let service = SandboxService::new(
        store.clone(),
        profile_store.clone(),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let first_operation = "00000000000040008000000000000001";
    let second_operation = "00000000000040008000000000000002";
    let first_request = admission(&profiles, &caller, "New agent", first_operation).await;
    let second_request = admission(&profiles, &caller, "New agent", second_operation).await;
    let mut legacy = first_request.clone();
    legacy.name = Some("New agent sandbox".into());
    legacy.origin = SandboxOrigin::Lifecycle {
        operation_id: "legacy-operation".into(),
    };
    service.create_for_lifecycle(&caller, legacy).await.unwrap();

    let first_lease = lease(first_operation);
    let second_lease = lease(second_operation);
    let (first, second, duplicate) = tokio::join!(
        create_sandbox(
            &service,
            &caller,
            first_request.profile.clone(),
            "New agent",
            &first_lease
        ),
        create_sandbox(
            &service,
            &caller,
            second_request.profile,
            "New agent",
            &second_lease
        ),
        create_sandbox(
            &service,
            &caller,
            first_request.profile,
            "New agent",
            &first_lease
        ),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_ne!(first.id, second.id);
    assert_ne!(first.name, second.name);
    assert_eq!(first, duplicate.unwrap());

    // No available quota or credentials are required to recover a committed
    // admission whose response or child-ID persistence was lost.
    let restarted = SandboxService::new(
        store.clone(),
        profile_store,
        SandboxPolicy {
            max_sandboxes_per_owner: 0,
            ..SandboxPolicy::default()
        },
        Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
    );
    for (operation_id, original) in [(first_operation, first), (second_operation, second)] {
        let replay = admission(&profiles, &caller, "New agent", operation_id).await;
        assert_eq!(
            create_sandbox(
                &restarted,
                &caller,
                replay.profile,
                "New agent",
                &lease(operation_id)
            )
            .await
            .unwrap(),
            original
        );
        assert_eq!(
            store
                .sandbox_events(&original.id, 0, 20)
                .await
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        store
            .list_sandboxes(&caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        3
    );
}

#[tokio::test]
async fn lifecycle_sandbox_names_preserve_full_operation_identity_with_maximum_utf8_titles() {
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let profiles = profiles(profile_store.clone());
    let service = SandboxService::new(
        Arc::new(InMemorySandboxStore::default()),
        profile_store,
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = owner();
    for (title, operation_id) in [
        ("a".repeat(64), "00000000000040008000000000000011"),
        ("测".repeat(26), "00000000000040008000000000000012"),
        ("🦀".repeat(20), "00000000000040008000000000000013"),
    ] {
        // Exercise the Sandbox's byte limit independently of the profile
        // catalog's separate display-name limit.
        let request = admission(&profiles, &caller, "Profile", operation_id).await;
        let sandbox = create_sandbox(
            &service,
            &caller,
            request.profile,
            &title,
            &lease(operation_id),
        )
        .await
        .unwrap();
        assert!(sandbox.name.len() <= 64);
        assert!(sandbox.name.ends_with(&format!(" sandbox {operation_id}")));
        assert!(title.starts_with(sandbox.name.split(" sandbox ").next().unwrap()));
    }
}

#[tokio::test]
async fn lifecycle_name_rollout_recovers_exact_legacy_admission_without_accepting_changed_requests()
{
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let profiles = profiles(profile_store.clone());
    let store = Arc::new(InMemorySandboxStore::default());
    let caller = owner();
    let service = SandboxService::new(
        store.clone(),
        profile_store.clone(),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let operation_id = "00000000000040008000000000000021";
    let mut old_request = admission(&profiles, &caller, "New agent", operation_id).await;
    old_request.name = Some("New agent sandbox".into());
    let original = service
        .create_for_lifecycle(&caller, old_request.clone())
        .await
        .unwrap();
    let restarted = SandboxService::new(
        store.clone(),
        profile_store,
        SandboxPolicy {
            max_sandboxes_per_owner: 0,
            ..SandboxPolicy::default()
        },
        Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
    );
    assert_eq!(
        create_sandbox(
            &restarted,
            &caller,
            old_request.profile.clone(),
            "New agent",
            &lease(operation_id)
        )
        .await
        .unwrap(),
        original
    );
    let mut changed_profile = old_request.profile.clone();
    changed_profile.revision += 1;
    for (profile, title) in [
        (changed_profile, "New agent"),
        (old_request.profile, "Different title"),
    ] {
        assert!(matches!(
            create_sandbox(&restarted, &caller, profile, title, &lease(operation_id)).await,
            Err(SandboxServiceError::SandboxOriginConflict)
        ));
    }
    assert_eq!(
        store
            .list_sandboxes(&caller.subject, &Default::default())
            .await
            .unwrap()
            .items,
        vec![original]
    );
}

#[tokio::test]
async fn lifecycle_name_rollout_preserves_name_conflict_from_another_operation() {
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let profiles = profiles(profile_store.clone());
    let store = Arc::new(InMemorySandboxStore::default());
    let service = SandboxService::new(
        store.clone(),
        profile_store,
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = owner();
    let operation_id = "00000000000040008000000000000031";
    let mut conflicting = admission(&profiles, &caller, "New agent", operation_id).await;
    let profile = conflicting.profile.clone();
    conflicting.origin = SandboxOrigin::Lifecycle {
        operation_id: "another-operation".into(),
    };
    let original = service
        .create_for_lifecycle(&caller, conflicting)
        .await
        .unwrap();
    assert!(matches!(
        create_sandbox(
            &service,
            &caller,
            profile,
            "New agent",
            &lease(operation_id)
        )
        .await,
        Err(SandboxServiceError::NameConflict)
    ));
    assert_eq!(
        store
            .list_sandboxes(&caller.subject, &Default::default())
            .await
            .unwrap()
            .items,
        vec![original]
    );
}

async fn create_sandbox(
    service: &layer2_core::sandbox::SandboxService,
    caller: &layer2_core::Caller,
    profile: layer2_core::profiles::SandboxProfileRef,
    agent_name: &str,
    lease: &LifecycleLease,
) -> Result<layer2_core::sandbox::Sandbox, SandboxServiceError> {
    create_sandbox_with_fence(service, caller, profile, agent_name, lease, None, None).await
}
