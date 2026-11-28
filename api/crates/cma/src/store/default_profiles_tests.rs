use super::*;

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    }
}

#[tokio::test]
async fn records_are_public_deterministic_and_reference_each_other() {
    let spec = DefaultProfileSpec {
        cpu_millis: 1_000,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    let first = spec.records(bounds(), &[]).await.unwrap();
    let second = spec.records(bounds(), &[]).await.unwrap();
    assert_eq!(first.agent, second.agent);
    assert_eq!(first.agent_revision, second.agent_revision);
    assert_eq!(first.workspace_revision, second.workspace_revision);
    assert_eq!(first.sandbox_revision, second.sandbox_revision);
    assert_eq!(first.agent.owner.as_str(), SYSTEM_OWNER);
    assert_eq!(first.agent.visibility, ProfileVisibility::Public);
    assert_eq!(
        first.agent.draft.permission_level,
        PermissionLevel::Autonomous
    );
    assert_eq!(first.agent.draft.thinking_effort, ThinkingEffort::High);
    assert_eq!(
        first
            .sandbox
            .draft
            .nyxid_services
            .iter()
            .map(layer2_core::ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["chrono-llm-public", "ornn-api"]
    );
    assert_eq!(
        first.workspace.draft.agent_profiles,
        vec![AgentProfileRef::new(
            first.agent.id,
            DEFAULT_PROFILE_REVISION
        )]
    );
    assert_eq!(
        first.sandbox.draft.workspace_profiles,
        vec![WorkspaceProfileRef::new(
            first.workspace.id,
            DEFAULT_PROFILE_REVISION
        )]
    );
}

#[tokio::test]
async fn used_default_profiles_remain_valid_without_accepting_configuration_drift() {
    let spec = DefaultProfileSpec {
        cpu_millis: 1_000,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    let expected = spec.records(bounds(), &[]).await.unwrap();
    for count in [1, 10, i64::MAX as u64] {
        let mut agent = expected.agent.clone();
        agent.uses_count = count;
        assert!(default_profile_matches(
            agent.clone(),
            &expected.agent,
            |p| &mut p.uses_count
        ));
        assert_eq!(agent.uses_count, count);
        agent.draft.model = Some("different-model".into());
        assert!(!default_profile_matches(agent, &expected.agent, |p| &mut p.uses_count));

        let mut workspace = expected.workspace.clone();
        workspace.uses_count = count;
        assert!(default_profile_matches(
            workspace.clone(),
            &expected.workspace,
            |p| &mut p.uses_count
        ));
        workspace.draft_version += 1;
        assert!(!default_profile_matches(
            workspace,
            &expected.workspace,
            |p| &mut p.uses_count
        ));

        let mut sandbox = expected.sandbox.clone();
        sandbox.uses_count = count;
        assert!(default_profile_matches(
            sandbox.clone(),
            &expected.sandbox,
            |p| &mut p.uses_count
        ));
        sandbox.draft.cpu_millis += 100;
        assert!(!default_profile_matches(sandbox, &expected.sandbox, |p| {
            &mut p.uses_count
        }));
    }
}

#[tokio::test]
async fn deployment_reservation_must_fit_publication_bounds() {
    let spec = DefaultProfileSpec {
        cpu_millis: 99,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    assert!(spec.records(bounds(), &[]).await.is_err());
}
