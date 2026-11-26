use super::*;
use layer2_core::profiles::{
    AgentProfileStore, InMemoryProfileStore, SandboxProfileStore, WorkspaceProfileStore,
    MAX_PROFILE_NAME_BYTES,
};

fn service(store: Arc<InMemoryProfileStore>) -> Arc<PrivateProfileCreationService> {
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

async fn materialize(
    profiles: &PrivateProfileCreationService,
    caller: &Caller,
    request: &AgentLifecycleCreateRequest,
) -> Result<
    (AgentProfileRevision, WorkspaceProfileRef, SandboxProfileRef),
    AgentLifecycleProvisioningError,
> {
    create_profiles(
        profiles,
        caller,
        request,
        None,
        "original-operation",
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

#[tokio::test]
async fn unicode_titles_recover_partial_profiles_and_replay_without_changing_agent_identity() {
    for (title, workspace_name, sandbox_name) in [
        (
            "a".repeat(64),
            format!("{} workspace", "a".repeat(64)),
            format!("{} sandbox", "a".repeat(64)),
        ),
        (
            "é".repeat(35),
            format!("{} workspace", "é".repeat(35)),
            format!("{} sandbox", "é".repeat(35)),
        ),
        (
            "测".repeat(26),
            format!("{} workspace", "测".repeat(23)),
            format!("{} sandbox", "测".repeat(24)),
        ),
        (
            "🦀".repeat(20),
            format!("{} workspace", "🦀".repeat(17)),
            format!("{} sandbox", "🦀".repeat(18)),
        ),
        (
            format!("{}a🦀", "é".repeat(34)),
            format!("{}a workspace", "é".repeat(34)),
            format!("{}a sandbox", "é".repeat(34)),
        ),
    ] {
        let store = Arc::new(InMemoryProfileStore::default());
        let profiles = service(store.clone());
        let caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
        let request: AgentLifecycleCreateRequest =
            serde_json::from_value(json!({"name":title})).unwrap();
        validate_request(&request).unwrap();
        // A previous worker can have created the Agent profile before failing
        // on the suffixed Workspace name. Its retained identity must replay.
        let retained_agent = profiles
            .create_agent_for_owner(
                &caller.subject,
                "agent-manager:original-operation:agent",
                title.clone(),
                "Inline Agent Manager configuration".into(),
                agent_content(&Default::default()).unwrap(),
            )
            .await
            .unwrap();
        let first = materialize(&profiles, &caller, &request).await.unwrap();
        assert_eq!(first.0, retained_agent);
        let stored_agent = store
            .agent_profile(&first.0.profile_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_agent.name, title);
        let workspace = store.workspace_profile(&first.1.id).await.unwrap().unwrap();
        let sandbox = store.sandbox_profile(&first.2.id).await.unwrap().unwrap();
        assert_eq!(workspace.name, workspace_name);
        assert_eq!(sandbox.name, sandbox_name);
        assert!(workspace.name.len() <= MAX_PROFILE_NAME_BYTES);
        assert!(sandbox.name.len() <= MAX_PROFILE_NAME_BYTES);

        let restarted = service(store.clone());
        let mut recovered: AgentLifecycleCreateRequest =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(
            materialize(&restarted, &caller, &recovered).await.unwrap(),
            first
        );
        // Display-name truncation cannot turn a changed request into a replay.
        recovered.name.pop();
        recovered.name.push('Z');
        validate_request(&recovered).unwrap();
        assert!(matches!(
            materialize(&restarted, &caller, &recovered).await,
            Err(AgentLifecycleProvisioningError::Conflict(_))
        ));
        assert_eq!(
            store
                .agent_profile(&first.0.profile_id)
                .await
                .unwrap()
                .unwrap(),
            stored_agent
        );
    }
}
