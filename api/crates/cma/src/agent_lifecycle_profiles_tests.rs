use super::*;
use layer2_core::profiles::{InMemoryProfileStore, SandboxProfileStore, WorkspaceProfileStore};
use layer3_adapter::SandboxCapacity;

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8000,
        min_memory_mib: 128,
        max_memory_mib: 16384,
    }
}

#[tokio::test]
async fn lifecycle_uses_captured_defaults_and_replays_profiles_after_configuration_changes() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let request = serde_json::from_value(serde_json::json!({"name":"Test Agent"})).unwrap();
    let input = LifecycleInput {
        behavior: LifecycleBehavior::PromptRepository,
        managed_agent_instructions: None,
        setup_message: None,
        request,
        resolved_image: None,
        capacity: Some(SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 4096,
        }),
        sandbox_availability: layer2_core::profiles::SandboxProfileAvailability {
            always_keep_alive: true,
            ..Default::default()
        },
        capabilities: vec![],
        actor: None,
    };
    let (_, _, first) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "original",
        input.captured_capacity(bounds()),
        input.captured_limits(),
        input.behavior,
        input.sandbox_availability,
        "",
    )
    .await
    .unwrap();
    let materialized = store
        .sandbox_profile_revision(&first.id, first.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            materialized.content.cpu_millis,
            materialized.content.memory_mib
        ),
        (2000, 4096)
    );
    assert_eq!(materialized.content.max_workspaces, 16);
    assert_eq!(materialized.content.max_agents_per_workspace, 16);
    let created = store.sandbox_profile(&first.id).await.unwrap().unwrap();
    assert!(created.internal);
    assert!(created.availability.always_keep_alive);
    assert!(!created.availability.warmup_enabled);
    let resumed: LifecycleInput =
        serde_json::from_slice(&serde_json::to_vec(&input).unwrap()).unwrap();
    let changed_bounds = ProfileCapacityBounds {
        min_cpu_millis: 3000,
        min_memory_mib: 8192,
        ..bounds()
    };
    assert_eq!(
        resumed.captured_capacity(changed_bounds),
        input.capacity.unwrap()
    );
    let (_, _, replayed) = create_profiles(
        &profiles,
        &caller,
        &resumed.request,
        None,
        "original",
        resumed.captured_capacity(changed_bounds),
        resumed.captured_limits(),
        resumed.behavior,
        resumed.sandbox_availability,
        "",
    )
    .await
    .unwrap();
    assert_eq!(replayed, first);
    assert_eq!(
        store.sandbox_profile(&first.id).await.unwrap().unwrap(),
        created
    );
}

#[tokio::test]
async fn retained_inline_profile_keeps_its_capacity_and_inactivity_policy_after_new_creation() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let old: LifecycleInput = serde_json::from_value(json!({
        "behavior":"prompt_repository",
        "request":{"name":"Retained Agent"},
        "resolved_image":null,
        "capacity":{"cpu_millis":1000,"memory_mib":1024}
    }))
    .unwrap();
    assert!(!old.sandbox_availability.always_keep_alive);
    let materialize = |key: &'static str, capacity, availability| {
        create_profiles(
            &profiles,
            &caller,
            &old.request,
            None,
            key,
            capacity,
            old.captured_limits(),
            old.behavior,
            availability,
            "",
        )
    };
    let (_, _, original) = materialize(
        "old",
        old.captured_capacity(bounds()),
        old.sandbox_availability,
    )
    .await
    .unwrap();
    let retained = store.sandbox_profile(&original.id).await.unwrap().unwrap();
    assert_eq!(
        (retained.draft.cpu_millis, retained.draft.memory_mib),
        (1000, 1024)
    );
    let (_, _, new) = materialize(
        "new",
        SandboxCapacity {
            cpu_millis: 2000,
            memory_mib: 4096,
        },
        layer2_core::profiles::SandboxProfileAvailability {
            always_keep_alive: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_ne!(original.id, new.id);
    assert!(
        store
            .sandbox_profile(&new.id)
            .await
            .unwrap()
            .unwrap()
            .availability
            .always_keep_alive
    );
    let (_, _, replayed) = materialize(
        "old",
        old.captured_capacity(bounds()),
        old.sandbox_availability,
    )
    .await
    .unwrap();
    assert_eq!(original, replayed);
    assert_eq!(
        store.sandbox_profile(&original.id).await.unwrap().unwrap(),
        retained
    );
}

#[tokio::test]
async fn legacy_lifecycle_without_capacity_replays_the_old_minimum_reservation() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let input: LifecycleInput = serde_json::from_value(serde_json::json!({
        "request":{"name":"Legacy Agent","repository":{"url":"https://github.com/org/legacy"}},"resolved_image":null
    }))
    .unwrap();
    assert!(input.capacity.is_none());
    assert!(!input.sandbox_availability.always_keep_alive);
    assert_eq!(input.behavior, LifecycleBehavior::Legacy);
    let (_, workspace, original) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "legacy",
        SandboxCapacity {
            cpu_millis: 100,
            memory_mib: 128,
        },
        input.captured_limits(),
        input.behavior,
        input.sandbox_availability,
        "",
    )
    .await
    .unwrap();
    let retained_workspace = store
        .workspace_profile_revision(&workspace.id, workspace.revision)
        .await
        .unwrap()
        .unwrap();
    assert!(retained_workspace.content.name.is_empty());
    assert!(matches!(
        retained_workspace.content.location,
        layer2_core::profiles::WorkspaceLocation::GitRepository { .. }
    ));
    let (_, _, replayed) = create_profiles(
        &profiles,
        &caller,
        &input.request,
        None,
        "legacy",
        input.captured_capacity(bounds()),
        input.captured_limits(),
        input.behavior,
        input.sandbox_availability,
        "",
    )
    .await
    .unwrap();
    assert_eq!(original, replayed);
    assert!(
        !store
            .sandbox_profile(&original.id)
            .await
            .unwrap()
            .unwrap()
            .availability
            .always_keep_alive
    );
    let retained = store
        .sandbox_profile_revision(&original.id, original.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.content.max_workspaces, 1);
    assert_eq!(retained.content.max_agents_per_workspace, 1);
    assert_eq!(
        (retained.content.cpu_millis, retained.content.memory_mib),
        (100, 128)
    );
}

#[tokio::test]
async fn repository_is_a_first_message_and_never_a_platform_clone_plan() {
    let store = Arc::new(InMemoryProfileStore::default());
    let profiles =
        PrivateProfileCreationService::new(store.clone(), bounds(), Arc::new([] as [String; 0]));
    let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    let request = serde_json::from_value(json!({"name":"Repo Agent","repository":{"url":"https://github.com/org/project","ref":"feature/setup"}})).unwrap();
    validate_request(&request).unwrap();
    let (agent, workspace, sandbox) = create_profiles(
        &profiles,
        &caller,
        &request,
        None,
        "repo-prompt",
        SandboxCapacity::default(),
        layer2_core::sandbox::SandboxLimits {
            max_workspaces: 16,
            max_agents_per_workspace: 16,
        },
        LifecycleBehavior::PromptRepository,
        Default::default(),
        "",
    )
    .await
    .unwrap();
    let workspace = store
        .workspace_profile_revision(&workspace.id, workspace.revision)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        workspace.content.location,
        layer2_core::profiles::WorkspaceLocation::Directory { .. }
    ));
    assert!(workspace.content.setup_commands.is_empty());
    assert!(agent.content.system_prompt.is_empty());
    assert_eq!(agent.content.model.as_deref(), Some("gpt-6"));
    assert_eq!(
        agent.content.thinking_effort,
        layer2_core::profiles::ThinkingEffort::High
    );
    assert_eq!(
        agent.content.permission_level,
        layer2_core::profiles::PermissionLevel::Autonomous
    );
    let sandbox = store
        .sandbox_profile_revision(&sandbox.id, sandbox.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sandbox.content.nyxid_services,
        crate::config::REQUIRED_BASELINE_SERVICES
            .iter()
            .copied()
            .map(layer2_core::ServiceId::new)
            .collect::<Vec<_>>()
    );
    let prompt = validation::setup_message(&request);
    assert!(prompt.contains("https://github.com/org/project"));
    assert!(prompt.contains("feature/setup"));
    assert!(prompt.contains("use gh api through CMA's api-github"));
    assert!(prompt.contains("Honor the requested ref; when absent, read default_branch"));
    assert!(prompt.contains("pin the returned commit SHA"));
    assert!(prompt.contains("Download /repos/{owner}/{repo}/tarball/{sha} with gh api"));
    assert!(prompt.contains("preserve existing files, local changes and .git metadata"));
    assert!(prompt.contains("request api-github through agent_key_scope_revision"));
    assert!(!prompt.contains("api-github-pat"));
    assert!(prompt.contains("Use API source checkout instead of gh repo clone"));
    assert!(prompt.contains("no Sandbox restart is needed"));
    assert!(prompt.contains("Do not ask the user to paste a secret"));
    let repeated = validation::setup_message(
        &serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap(),
    );
    assert_eq!(repeated, prompt);
}

#[tokio::test]
async fn machine_included_profiles_pin_the_desktop_image_and_marker() {
    for desktop in [false, true] {
        let store = Arc::new(InMemoryProfileStore::default());
        let profiles = PrivateProfileCreationService::new(
            store.clone(),
            bounds(),
            Arc::new([] as [String; 0]),
        );
        let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
        let request = serde_json::from_value(json!({
            "name": "Desktop Agent",
            "machine_included": true,
            "desktop": desktop,
            "initial_message": "  Keep the exact task\nwith both independent options  "
        }))
        .unwrap();
        validate_request(&request).unwrap();
        assert_eq!(request.desktop, desktop);
        assert!(request.machine_included);
        assert_eq!(
            validation::admitted_first_message(&LifecycleInput {
                behavior: LifecycleBehavior::PromptRepository,
                managed_agent_instructions: None,
                setup_message: Some("Readiness is not the task".into()),
                request: request.clone(),
                resolved_image: None,
                capacity: None,
                sandbox_availability: Default::default(),
                capabilities: vec![],
                actor: None,
            })
            .as_deref(),
            Some("  Keep the exact task\nwith both independent options  ")
        );
        let (_, _, sandbox_ref) = create_profiles(
        &profiles,
        &caller,
        &request,
        Some("ghcr.io/chronoaiproject/cma-runtime-desktop@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        "desktop",
        SandboxCapacity { cpu_millis: 2_000, memory_mib: 8_192 },
        layer2_core::sandbox::SandboxLimits { max_workspaces: 16, max_agents_per_workspace: 16 },
        LifecycleBehavior::PromptRepository,
        Default::default(),
        "",
    )
    .await
    .unwrap();
        let sandbox = store
            .sandbox_profile_revision(&sandbox_ref.id, sandbox_ref.revision)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sandbox.content.runtime_image.as_deref(), Some("ghcr.io/chronoaiproject/cma-runtime-desktop@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert_eq!(sandbox.content.cpu_millis, 2_000);
        assert_eq!(sandbox.content.memory_mib, 8_192);
        assert_eq!(
            sandbox.content.environment_variables,
            vec![layer2_core::profiles::EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into()
            }]
        );
    }
}

#[test]
fn empty_workspace_setup_waits_for_user_tasks_without_optional_access_requests() {
    for context in [
        json!({}),
        json!({"workspace_id":"existing"}),
        json!({"sandbox_id":"existing"}),
    ] {
        let mut request = json!({"name":"Empty workspace"});
        request
            .as_object_mut()
            .unwrap()
            .extend(context.as_object().unwrap().clone());
        let request = serde_json::from_value(request).unwrap();
        let prompt = validation::setup_message(&request);
        assert!(prompt.contains("Preserve existing files and changes"));
        assert!(prompt.contains("local workspace readiness only"));
        assert!(prompt.contains("no repository or external-service task has been requested"));
        assert!(prompt.contains("Do not probe GitHub or other optional services, or request access merely for readiness"));
        assert!(prompt.contains("wait for the user's next task"));
        assert!(!prompt.contains("agent_key_scope_revision"));
        assert!(!prompt.contains("The user selected this GitHub repository"));
        // Later user-requested GitHub work still has the fixed API/MCP guidance.
        let profile = guidance::provisioning_request(&request, Some(guidance::managed_v2()));
        let instructions = profile.profile.unwrap().system_prompt.unwrap();
        assert!(instructions.contains("POST /user/repos or /orgs/{org}/repos"));
        assert!(instructions.contains("agent_key_scope_revision"));
        assert!(instructions.contains("tarball/{sha}"));
    }
}

#[test]
fn admitted_setup_replays_exact_captured_message_and_legacy_fallback() {
    for repository in [
        json!(null),
        json!({"url":"https://github.com/org/project","ref":"feature/setup"}),
    ] {
        for instructions in [
            None,
            Some(guidance::GITHUB_API_V1),
            Some(guidance::managed_v2()),
        ] {
            let mut old: LifecycleInput = serde_json::from_value(json!({
                "behavior":"prompt_repository", "managed_agent_instructions":instructions,
                "request":{"name":"Admitted", "repository":repository}, "resolved_image":null
            }))
            .unwrap();
            assert!(old.setup_message.is_none());
            let original = validation::first_message(&old.request);
            assert_eq!(
                validation::admitted_first_message(&old),
                Some(original.clone())
            );
            // Exact admitted bytes win over a later deployment's prompt builder.
            old.setup_message = Some("Original admitted setup\nRetain these bytes.\n".into());
            let restored: LifecycleInput =
                serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
            assert_eq!(
                validation::admitted_first_message(&restored),
                old.setup_message
            );
            assert_ne!(
                validation::admitted_first_message(&restored),
                Some(original)
            );
            old.behavior = LifecycleBehavior::Legacy;
            assert_eq!(validation::admitted_first_message(&old), None);
        }
    }
}

#[test]
fn legacy_operations_retain_original_setup_and_permissions_after_upgrade() {
    let input: LifecycleInput = serde_json::from_value(json!({
        "request":{"name":"Old pending Agent","repository":{"url":"https://github.com/org/project"}},
        "resolved_image":null
    })).unwrap();
    assert_eq!(input.behavior, LifecycleBehavior::Legacy);
    let profile = input.request.profile.clone().unwrap_or_default();
    let original = validation::agent_content_for(&profile, input.behavior).unwrap();
    assert_eq!(original.model, None);
    assert_eq!(
        original.thinking_effort,
        layer2_core::profiles::ThinkingEffort::Medium
    );
    assert_eq!(
        original.permission_level,
        layer2_core::profiles::PermissionLevel::Guarded
    );
    assert!(
        validation::services_for(&profile.permissions, input.behavior)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        validation::repository_location(&input.request, input.behavior, "").unwrap(),
        layer2_core::profiles::WorkspaceLocation::GitRepository { .. }
    ));
    let admitted: LifecycleInput = serde_json::from_value(json!({
        "behavior":"prompt_repository","request":input.request,"resolved_image":null
    }))
    .unwrap();
    let recovered: LifecycleInput =
        serde_json::from_slice(&serde_json::to_vec(&admitted).unwrap()).unwrap();
    assert_eq!(recovered.behavior, LifecycleBehavior::PromptRepository);
    assert!(matches!(
        validation::repository_location(&recovered.request, recovered.behavior, "").unwrap(),
        layer2_core::profiles::WorkspaceLocation::Directory { .. }
    ));
}
