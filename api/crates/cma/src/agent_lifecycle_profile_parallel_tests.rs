use super::*;
use layer2_core::profiles::{
    content_digest, AgentProfile, AgentProfileStore, InMemoryProfileStore, ProfilePage,
    ProfilePageRequest, ProfileStoreError, ProfileVisibility, PublicValueScanner, SandboxProfile,
    SandboxProfileRevision, SandboxProfileStore, WorkspaceProfile, WorkspaceProfileRevision,
    WorkspaceProfileStore,
};
use layer2_core::AgentProfileId;

#[path = "agent_lifecycle_profile_parallel_tests/store.rs"]
mod store;
use store::Store;

type Created = (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef);
const INITIAL: [&str; 3] = [
    "ensure_initial_agent_profile",
    "ensure_initial_workspace_profile",
    "ensure_initial_sandbox_profile",
];

fn service(store: Arc<Store>) -> Arc<PrivateProfileCreationService> {
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

async fn create(
    profiles: &PrivateProfileCreationService,
    owner: &str,
    title: &str,
) -> Result<Created, AgentLifecycleProvisioningError> {
    create_profiles(
        profiles,
        &Caller::new(SubjectId::new(owner), Authorization::new([])),
        &serde_json::from_value(json!({"name": title})).unwrap(),
        None,
        "same-durable-operation",
        layer3_adapter::SandboxCapacity::default(),
        layer2_core::sandbox::SandboxLimits {
            max_workspaces: 16,
            max_agents_per_workspace: 16,
        },
        LifecycleBehavior::PromptRepository,
        Default::default(),
        "",
    )
    .await
}

async fn assert_revisions(store: &Store, created: &Created) {
    let page = ProfilePageRequest::default();
    let agent = store
        .inner
        .list_agent_profile_revisions(&created.0.profile_id, &page)
        .await
        .unwrap();
    let workspace = store
        .inner
        .list_workspace_profile_revisions(&created.1.id, &page)
        .await
        .unwrap();
    let sandbox = store
        .inner
        .list_sandbox_profile_revisions(&created.2.id, &page)
        .await
        .unwrap();
    assert_eq!(agent.items, vec![created.0.clone()]);
    assert_eq!(workspace.items.len(), 1);
    assert_eq!(sandbox.items.len(), 1);
    assert_eq!(workspace.items[0].revision, created.1.revision);
    assert_eq!(sandbox.items[0].revision, created.2.revision);
    assert_eq!(created.1.revision, 1);
    assert_eq!(created.2.revision, 1);
    assert!(workspace.items[0].content.agent_profiles.is_empty());
    assert_eq!(
        sandbox.items[0].content.workspace_profiles,
        vec![created.1.clone()]
    );
    assert_eq!(
        created.0.digest,
        content_digest(&created.0.content).unwrap()
    );
    assert_eq!(
        workspace.items[0].digest,
        content_digest(&workspace.items[0].content).unwrap()
    );
    assert_eq!(
        sandbox.items[0].digest,
        content_digest(&sandbox.items[0].content).unwrap()
    );
}

#[tokio::test]
async fn workspace_and_sandbox_finish_while_agent_storage_is_blocked() {
    let store = Arc::new(Store::blocking_agent());
    let profiles = service(store.clone());
    // The sequential implementation cannot complete: Agent insertion waits for
    // Sandbox publication. This proves actual independent branch progress.
    let created = tokio::time::timeout(
        Duration::from_secs(2),
        create(&profiles, "owner", "Parallel Agent"),
    )
    .await
    .expect("Workspace/Sandbox must run while Agent creation is pending")
    .unwrap();
    assert_revisions(&store, &created).await;
    let events = store.events.lock().unwrap();
    let at = |name, after| {
        events
            .iter()
            .position(|event| *event == (name, after))
            .unwrap()
    };
    assert!(at("ensure_initial_workspace_profile", true) < at("create_sandbox_profile", false));
    assert!(at("ensure_initial_sandbox_profile", true) < at("create_agent_profile", true));
    for method in INITIAL {
        assert_eq!(
            events
                .iter()
                .filter(|event| **event == (method, true))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn each_branch_failure_finishes_other_work_and_replays_exact_private_profiles() {
    for method in [
        "create_agent_profile",
        "ensure_initial_agent_profile",
        "create_workspace_profile",
        "ensure_initial_workspace_profile",
        "create_sandbox_profile",
        "ensure_initial_sandbox_profile",
    ] {
        let store = Arc::new(Store::default());
        store.fail(
            method,
            false,
            ProfileStoreError::Unavailable("injected".into()),
        );
        assert!(matches!(
            create(&service(store.clone()), "owner", "Retry Agent").await,
            Err(AgentLifecycleProvisioningError::Unavailable)
        ));
        if method.contains("agent") {
            assert_eq!(store.completed(INITIAL[1]), 1);
            assert_eq!(store.completed(INITIAL[2]), 1);
        } else {
            assert_eq!(store.completed(INITIAL[0]), 1);
        }
        if method.contains("workspace") {
            assert_eq!(store.completed("create_sandbox_profile"), 0);
        }
        // Reconstruct the service to model a fresh worker resuming durable IDs.
        let created = create(&service(store.clone()), "owner", "Retry Agent")
            .await
            .unwrap();
        assert_revisions(&store, &created).await;
        assert_eq!(
            create(&service(store.clone()), "owner", "Retry Agent")
                .await
                .unwrap(),
            created
        );
        // Ensuring the same initial publication may execute on every replay.
        // Its durable history, checked after the final retry, must still have
        // exactly one revision of the original content for every profile kind.
        assert_revisions(&store, &created).await;
    }
}

#[tokio::test]
async fn lost_publication_replies_reuse_committed_revisions_in_both_branches() {
    for method in INITIAL {
        let store = Arc::new(Store::default());
        store.fail(
            method,
            true,
            ProfileStoreError::Unavailable("reply lost".into()),
        );
        assert!(matches!(
            create(&service(store.clone()), "owner", "Lost Reply").await,
            Err(AgentLifecycleProvisioningError::Unavailable)
        ));
        let created = create(&service(store.clone()), "owner", "Lost Reply")
            .await
            .unwrap();
        // Ensuring the same initial publication may execute on every replay.
        // Its durable history, checked after the final retry, must still have
        // exactly one revision of the original content for every profile kind.
        assert_revisions(&store, &created).await;
    }
}

#[tokio::test]
async fn simultaneous_failures_keep_agent_first_error_precedence() {
    for agent_unavailable in [true, false] {
        let store = Arc::new(Store::default());
        let unavailable = ProfileStoreError::Unavailable("injected".into());
        let protected = ProfileStoreError::Protected;
        let (agent_error, workspace_error) = if agent_unavailable {
            (unavailable, protected)
        } else {
            (protected, unavailable)
        };
        store.fail("create_agent_profile", false, agent_error);
        store.fail("create_workspace_profile", false, workspace_error);
        let result = create(&service(store.clone()), "owner", "Both Failed").await;
        if agent_unavailable {
            assert!(matches!(
                result,
                Err(AgentLifecycleProvisioningError::Unavailable)
            ));
        } else {
            assert!(
                matches!(result, Err(AgentLifecycleProvisioningError::Invalid(detail))
                if detail == ProfileStoreError::Protected.to_string())
            );
        }
        assert!(store
            .events
            .lock()
            .unwrap()
            .contains(&("create_workspace_profile", false)));
        assert_eq!(store.completed("create_sandbox_profile"), 0);
        let recovered = create(&service(store.clone()), "owner", "Both Failed")
            .await
            .unwrap();
        assert_revisions(&store, &recovered).await;
    }
}

#[tokio::test]
async fn parallel_profiles_retain_owner_isolation_and_reject_changed_identity() {
    let store = Arc::new(Store::default());
    let profiles = service(store.clone());
    let first = create(&profiles, "owner-a", "Original").await.unwrap();
    assert!(matches!(
        create(&profiles, "owner-a", "Changed").await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    assert_eq!(
        create(&profiles, "owner-a", "Original").await.unwrap(),
        first
    );
    let second = create(&profiles, "owner-b", "Original").await.unwrap();
    assert_ne!(first.0.profile_id, second.0.profile_id);
    assert_ne!(first.1.id, second.1.id);
    assert_ne!(first.2.id, second.2.id);
    for (owner, created) in [("owner-a", first), ("owner-b", second)] {
        let agent = store
            .inner
            .agent_profile(&created.0.profile_id)
            .await
            .unwrap()
            .unwrap();
        let workspace = store
            .inner
            .workspace_profile(&created.1.id)
            .await
            .unwrap()
            .unwrap();
        let sandbox = store
            .inner
            .sandbox_profile(&created.2.id)
            .await
            .unwrap()
            .unwrap();
        for (subject, visibility, internal) in [
            (agent.owner, agent.visibility, agent.internal),
            (workspace.owner, workspace.visibility, workspace.internal),
            (sandbox.owner, sandbox.visibility, sandbox.internal),
        ] {
            assert_eq!(subject.as_str(), owner);
            assert_eq!(visibility, ProfileVisibility::Private);
            assert!(internal);
        }
        assert_revisions(&store, &created).await;
    }
}
