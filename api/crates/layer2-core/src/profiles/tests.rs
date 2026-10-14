use super::*;
use crate::ids::{
    AgentProfileId, SandboxProfileId, ServiceId, SkillId, SubjectId, WorkspaceProfileId,
};

const NOW: &str = "2026-09-05T00:00:00Z";

#[path = "availability_tests.rs"]
mod availability_tests;
#[path = "connection_tests.rs"]
mod connection_tests;
#[path = "reference_tests.rs"]
mod reference_tests;

fn agent_profile(id: &str, owner: &str, visibility: ProfileVisibility) -> AgentProfile {
    AgentProfile {
        internal: false,
        id: AgentProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Research agent".into(),
        description: "A careful research assistant".into(),
        visibility,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-5".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![],
            system_prompt: "Be precise.".into(),
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn workspace_profile(id: &str, owner: &str, agent: AgentProfileRef) -> WorkspaceProfile {
    WorkspaceProfile {
        internal: false,
        id: WorkspaceProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Research workspace".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::GitRepository {
                url: "https://github.com/the-omega-institute/trureturing".into(),
                reference: None,
            },
            name: "trureturing".into(),
            setup_commands: vec!["lake build".into()],
            agent_profiles: vec![agent],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn sandbox_profile(id: &str, owner: &str, workspace: WorkspaceProfileRef) -> SandboxProfile {
    SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: SandboxProfileId(id.into()),
        owner: SubjectId(owner.into()),
        name: "Lean research".into(),
        description: String::new(),
        visibility: ProfileVisibility::Public,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 2_000,
            memory_mib: 4_096,
            nyxid_services: vec![ServiceId::new("ornn-api")],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: vec![workspace],
            software_install_commands: vec!["curl -fsSL https://example.invalid/lean | sh".into()],
            environment_variables: vec![EnvironmentVariable {
                name: "LEAN_VERSION".into(),
                value: "4.20.0".into(),
            }],
        },
        draft_version: 1,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
    }
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    }
}

#[test]
fn canonical_digests_are_stable_for_every_profile_kind() {
    let agent = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    let workspace = workspace_profile(
        "wkp_one",
        "owner",
        AgentProfileRef::new(agent.id.clone(), 1),
    );
    let sandbox = sandbox_profile(
        "sbp_one",
        "owner",
        WorkspaceProfileRef::new(workspace.id.clone(), 1),
    );
    assert_eq!(
        content_digest(&agent.draft).unwrap(),
        "5e2334daa625e6e5585e25b300ddc8cd822259ac89105909f71acc34eda2c569"
    );
    assert_eq!(
        content_digest(&workspace.draft).unwrap(),
        "1f42247bbb8b81420cf491379ab5414c6923a0c717f96b73a2aecae20da251a0"
    );
    assert_eq!(
        content_digest(&sandbox.draft).unwrap(),
        "4f97af16a5a8a6c65e2c7959a61b74f5b3647c6b6939bf2871fcb2a056aa3a0a"
    );
}

#[test]
fn workspace_git_urls_reject_an_explicit_default_port() {
    let mut profile = workspace_profile(
        "wkp_port",
        "owner",
        AgentProfileRef::new(AgentProfileId("agp_port".into()), 1),
    );
    profile.draft.location = WorkspaceLocation::GitRepository {
        url: "https://github.com:443/example/repository".into(),
        reference: None,
    };
    assert_eq!(
        profile.validate(),
        Err(ProfileError::new("invalid_repository", "location.url"))
    );
}

#[tokio::test]
async fn validation_refuses_reserved_environment_names_and_secret_like_values() {
    let mut profile = sandbox_profile(
        "sbp_one",
        "owner",
        WorkspaceProfileRef::new(WorkspaceProfileId("wkp_one".into()), 1),
    );
    profile.draft.environment_variables[0].name = "CHRONO_TOKEN".into();
    assert_eq!(
        profile.draft.validate(bounds()).unwrap_err().code,
        "reserved_environment_variable"
    );

    profile.draft.environment_variables[0] = EnvironmentVariable {
        name: "PUBLIC_SETTING".into(),
        value: "contains-private-marker".into(),
    };
    let error = profile
        .draft
        .validate_public_values(&["private-marker".into()])
        .await
        .unwrap_err();
    assert_eq!(error.code, "secret_like_environment_value");
}

#[test]
fn validation_protects_platform_and_selected_provider_environment_names() {
    let mut profile = sandbox_profile(
        "sbp_one",
        "owner",
        WorkspaceProfileRef::new(WorkspaceProfileId("wkp_one".into()), 1),
    );
    let provider_names = profile.draft.agent_provider.reserved_environment_names();
    assert!(!provider_names.is_empty());
    for name in [
        "PATH",
        "HOME",
        "NYXID_URL",
        "NYXID_ACCESS_TOKEN",
        "CHRONO_SETTING",
    ]
    .into_iter()
    .chain(provider_names.iter().copied())
    {
        profile.draft.environment_variables[0].name = name.into();
        assert_eq!(
            profile.draft.validate(bounds()),
            Err(ProfileError::new(
                "reserved_environment_variable",
                "environment_variables.name",
            )),
            "{name}"
        );
    }
    profile.draft.environment_variables[0].name = "PUBLIC_SETTING".into();
    assert_eq!(profile.draft.validate(bounds()), Ok(()));
}

#[test]
fn commands_accept_single_lines_or_complete_heredocs_only() {
    validate_command("curl -fsSL https://example.invalid/install | sh").unwrap();
    validate_command("cat <<'EOF' > tool.conf\nmode=proof\nEOF\nchmod 600 tool.conf").unwrap();
    validate_command("cat <<-\"END\" > tool.conf\n\tmode=proof\n\tEND").unwrap();

    for invalid in [
        "echo first\necho second",
        "cat <<EOF\nunterminated",
        "cat <<''\nvalue",
    ] {
        assert_eq!(
            validate_command(invalid).unwrap_err(),
            ProfileError::new("invalid_command", "commands")
        );
    }
}

#[test]
fn all_profile_content_refuses_unknown_fields() {
    let json =
        serde_json::to_value(agent_profile("agp_one", "owner", ProfileVisibility::Private).draft)
            .unwrap();
    let mut object = json.as_object().unwrap().clone();
    object.insert("future".into(), serde_json::json!(true));
    assert!(serde_json::from_value::<AgentProfileContent>(object.into()).is_err());

    let skill = SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new("proof-search"),
        version: Some("1".into()),
    };
    assert_eq!(
        serde_json::from_str::<SkillRef>(&serde_json::to_string(&skill).unwrap()).unwrap(),
        skill
    );
}

#[tokio::test]
async fn every_profile_aggregate_round_trips_and_refuses_unknown_fields() {
    fn round_trip<T>(value: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(serde_json::from_value::<T>(json.clone()).unwrap(), *value);
        let mut object = json.as_object().unwrap().clone();
        object.insert("unknown".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<T>(object.into()).is_err());
    }

    let agent = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    let agent_revision = agent.publish(1, NOW.into()).unwrap();
    round_trip(&agent);
    round_trip(&agent_revision);
    round_trip(&AgentProfileSnapshot::from(&agent_revision));

    let workspace = workspace_profile(
        "wkp_one",
        "owner",
        AgentProfileRef::new(agent.id.clone(), 1),
    );
    let workspace_revision = workspace.publish(1, NOW.into()).unwrap();
    round_trip(&workspace);
    round_trip(&workspace_revision);
    round_trip(&WorkspaceProfileSnapshot::from(&workspace_revision));

    let sandbox = sandbox_profile(
        "sbp_one",
        "owner",
        WorkspaceProfileRef::new(workspace.id, 1),
    );
    let sandbox_revision = sandbox.publish(1, NOW.into(), bounds(), &[]).await.unwrap();
    round_trip(&sandbox);
    round_trip(&sandbox_revision);
    round_trip(&SandboxProfileSnapshot::from(&sandbox_revision));
}

#[tokio::test]
async fn revisions_are_snapshots_and_later_draft_edits_do_not_change_them() {
    let store = InMemoryProfileStore::default();
    let mut profile = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(profile.clone()).await.unwrap();
    let revision = store
        .publish_agent_profile(&profile.id, 1, NOW.into())
        .await
        .unwrap();
    let snapshot = AgentProfileSnapshot::from(&revision);

    profile.draft.system_prompt = "Changed later".into();
    profile.draft_version = 2;
    store.save_agent_draft(1, profile).await.unwrap();
    let stored = store
        .agent_profile_revision(&snapshot.id, snapshot.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(AgentProfileSnapshot::from(&stored), snapshot);
    assert_eq!(snapshot.content.system_prompt, "Be precise.");
}

#[tokio::test]
async fn duplicate_profile_ids_do_not_replace_existing_records() {
    let store = InMemoryProfileStore::default();
    let original = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(original.clone()).await.unwrap();

    let mut duplicate = original.clone();
    duplicate.name = "Replacement".into();
    assert_eq!(
        store.create_agent_profile(duplicate).await.unwrap_err(),
        ProfileStoreError::Duplicate
    );

    let stored = store
        .list_agent_profiles(Some(&original.owner), false, &ProfilePageRequest::default())
        .await
        .unwrap();
    assert_eq!(stored.items, vec![original]);
}

#[tokio::test]
async fn draft_cas_preserves_profile_control_fields_and_handles_overflow() {
    let store = InMemoryProfileStore::default();
    let original = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(original.clone()).await.unwrap();

    let mut candidates = Vec::new();
    let mut candidate = original.clone();
    candidate.owner = SubjectId("other".into());
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.visibility = ProfileVisibility::Public;
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.forked_from = Some(AgentProfileRef::new(AgentProfileId("agp_source".into()), 1));
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.uses_count = 1;
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.created_at = "changed".into();
    candidates.push(candidate);
    let mut candidate = original.clone();
    candidate.retired_at = Some("changed".into());
    candidates.push(candidate);

    for mut candidate in candidates {
        candidate.draft_version = 2;
        assert_eq!(
            store.save_agent_draft(1, candidate).await.unwrap_err(),
            ProfileStoreError::Conflict
        );
    }

    let mut updated = original.clone();
    updated.draft.system_prompt = "Updated".into();
    updated.draft_version = 2;
    store.save_agent_draft(1, updated.clone()).await.unwrap();
    let stored = store
        .list_agent_profiles(Some(&original.owner), false, &ProfilePageRequest::default())
        .await
        .unwrap();
    assert_eq!(stored.items, vec![updated]);

    let mut saturated = agent_profile("agp_max", "owner", ProfileVisibility::Private);
    saturated.draft_version = u64::MAX;
    store.create_agent_profile(saturated.clone()).await.unwrap();
    assert_eq!(
        store
            .save_agent_draft(u64::MAX, saturated)
            .await
            .unwrap_err(),
        ProfileStoreError::Conflict
    );
}

#[tokio::test]
async fn revision_pages_are_bounded_and_newest_first() {
    let store = InMemoryProfileStore::default();
    let mut profile = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(profile.clone()).await.unwrap();
    store
        .publish_agent_profile(&profile.id, 1, "2026-09-05T00:00:00Z".into())
        .await
        .unwrap();
    profile.draft_version = 2;
    profile.draft.system_prompt = "Second".into();
    store.save_agent_draft(1, profile.clone()).await.unwrap();
    store
        .publish_agent_profile(&profile.id, 2, "2026-09-05T01:00:00Z".into())
        .await
        .unwrap();

    let first = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items[0].revision, 2);
    let second = store
        .list_agent_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: first.next,
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].revision, 1);
    assert!(second.next.is_none());
}

#[tokio::test]
async fn references_must_resolve_to_own_or_public_revisions() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_public", "publisher", ProfileVisibility::Public);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();

    let workspace = workspace_profile(
        "wkp_consumer",
        "consumer",
        AgentProfileRef::new(agent.id.clone(), 1),
    );
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();

    let private_agent = agent_profile("agp_private", "publisher", ProfileVisibility::Private);
    store
        .create_agent_profile(private_agent.clone())
        .await
        .unwrap();
    store
        .publish_agent_profile(&private_agent.id, 1, NOW.into())
        .await
        .unwrap();
    let inaccessible = workspace_profile(
        "wkp_bad",
        "consumer",
        AgentProfileRef::new(private_agent.id, 1),
    );
    store
        .create_workspace_profile(inaccessible.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&inaccessible.id, 1, NOW.into())
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
}

#[tokio::test]
async fn sandbox_publication_resolves_workspace_and_checks_secrets() {
    let store = InMemoryProfileStore::default();
    let owner = "owner";
    let agent = agent_profile("agp_one", owner, ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();
    let workspace = workspace_profile("wkp_one", owner, AgentProfileRef::new(agent.id.clone(), 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();
    let mut sandbox = sandbox_profile("sbp_one", owner, WorkspaceProfileRef::new(workspace.id, 1));
    sandbox.draft.environment_variables[0].value = "credential-shape".into();
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_sandbox_profile(
                &sandbox.id,
                1,
                NOW.into(),
                bounds(),
                &["credential-shape".into()],
            )
            .await
            .unwrap_err(),
        ProfileStoreError::Validation(ProfileError::environment_secret("LEAN_VERSION"))
    );
}

#[tokio::test]
async fn marketplace_lists_searches_reads_and_pages_public_profiles() {
    let store = InMemoryProfileStore::default();
    for (id, name, published_at) in [
        ("agp_one", "Alpha research", "2026-09-05T00:00:00Z"),
        ("agp_two", "Beta coding", "2026-09-05T01:00:00Z"),
    ] {
        let mut profile = agent_profile(id, "publisher", ProfileVisibility::Public);
        profile.name = name.into();
        store.create_agent_profile(profile.clone()).await.unwrap();
        store
            .publish_agent_profile(&profile.id, 1, published_at.into())
            .await
            .unwrap();
    }
    let first = store
        .list_marketplace(&MarketplaceQuery {
            limit: 1,
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(first.items[0].name, "Beta coding");
    let second = store
        .list_marketplace(&MarketplaceQuery {
            cursor: first.next,
            limit: 1,
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(second.items[0].name, "Alpha research");

    let search = store
        .list_marketplace(&MarketplaceQuery {
            text: Some("alpha".into()),
            ..MarketplaceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(search.items.len(), 1);
    let entry = store
        .read_marketplace(&MarketplaceLocator::Agent(AgentProfileId("agp_one".into())))
        .await
        .unwrap();
    assert!(matches!(entry.revision, MarketplaceRevision::Agent(_)));
}

#[tokio::test]
async fn visibility_requires_a_published_valid_revision() {
    let store = InMemoryProfileStore::default();
    let profile = agent_profile("agp_one", "owner", ProfileVisibility::Private);
    store.create_agent_profile(profile.clone()).await.unwrap();
    let error = store
        .set_profile_visibility(
            &profile.owner,
            &MarketplaceLocator::Agent(profile.id.clone()),
            ProfileVisibility::Public,
            NOW.into(),
            bounds(),
            &[],
        )
        .await
        .unwrap_err();
    assert_eq!(error, MarketplaceError::Unpublished);
    store
        .publish_agent_profile(&profile.id, 1, NOW.into())
        .await
        .unwrap();
    store
        .set_profile_visibility(
            &profile.owner,
            &MarketplaceLocator::Agent(profile.id.clone()),
            ProfileVisibility::Public,
            NOW.into(),
            bounds(),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .list_marketplace(&MarketplaceQuery::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn fork_copies_revision_into_private_draft_with_provenance() {
    let store = InMemoryProfileStore::default();
    let source = agent_profile("agp_source", "publisher", ProfileVisibility::Public);
    store.create_agent_profile(source.clone()).await.unwrap();
    store
        .publish_agent_profile(&source.id, 1, NOW.into())
        .await
        .unwrap();
    let forked = store
        .fork_profile(
            &SubjectId("consumer".into()),
            &MarketplaceLocator::Agent(source.id.clone()),
            1,
            NOW.into(),
        )
        .await
        .unwrap();
    let ForkedProfile::Agent(forked) = forked else {
        panic!("wrong fork kind");
    };
    assert_eq!(forked.owner, SubjectId("consumer".into()));
    assert_eq!(forked.visibility, ProfileVisibility::Private);
    assert_eq!(forked.draft, source.draft);
    assert_eq!(forked.forked_from, Some(AgentProfileRef::new(source.id, 1)));
}

#[test]
fn trust_confirmation_is_bound_to_the_displayed_revision_digest() {
    let publisher = SubjectId("publisher".into());
    let consumer = SubjectId("consumer".into());
    assert_eq!(
        require_trust_confirmation(&publisher, &consumer, true, "digest-one", None),
        Err(MarketplaceError::TrustConfirmationRequired {
            commands_digest: "digest-one".into()
        })
    );
    assert_eq!(
        require_trust_confirmation(
            &publisher,
            &consumer,
            true,
            "digest-one",
            Some("digest-two")
        ),
        Err(MarketplaceError::TrustConfirmationRequired {
            commands_digest: "digest-one".into()
        })
    );
    assert!(require_trust_confirmation(
        &publisher,
        &consumer,
        true,
        "digest-one",
        Some("digest-one")
    )
    .is_ok());
    assert_eq!(
        require_trust_confirmation(&publisher, &publisher, true, "digest-one", None),
        Ok(None)
    );
}

#[tokio::test]
async fn retirement_hides_marketplace_but_preserves_revision_snapshot() {
    let store = InMemoryProfileStore::default();
    let source = agent_profile("agp_source", "publisher", ProfileVisibility::Public);
    store.create_agent_profile(source.clone()).await.unwrap();
    let revision = store
        .publish_agent_profile(&source.id, 1, NOW.into())
        .await
        .unwrap();
    store
        .retire_agent_profile(&source.id, &source.owner, "2026-09-06T00:00:00Z".into())
        .await
        .unwrap();
    assert!(store
        .list_marketplace(&MarketplaceQuery::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .list_agent_profiles(None, true, &ProfilePageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        store.agent_profile_revision(&source.id, 1).await.unwrap(),
        Some(revision)
    );
}
