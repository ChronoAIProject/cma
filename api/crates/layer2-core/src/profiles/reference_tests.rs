use super::*;

#[test]
fn inherited_evidence_rejects_wrong_identity_and_corrupt_content() {
    let source = workspace_profile(
        "wkp_evidence",
        "publisher",
        AgentProfileRef::new(AgentProfileId("agp_child".into()), 1),
    );
    let revision = source.publish(1, NOW.into()).unwrap();
    let mut parent = source.clone();
    parent.id = WorkspaceProfileId("wkp_fork".into());
    parent.owner = SubjectId("consumer".into());
    let mut inherited = references::InheritedReferences::default();
    assert_eq!(
        inherited.inherit_workspace(&parent, &revision).unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    parent.forked_from = Some(WorkspaceProfileRef::new(source.id, 1));
    let mut wrong_revision = revision.clone();
    wrong_revision.revision = 2;
    assert_eq!(
        inherited
            .inherit_workspace(&parent, &wrong_revision)
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    let mut corrupt = revision.clone();
    corrupt.content.agent_profiles[0].revision = 2;
    assert!(matches!(
        inherited.inherit_workspace(&parent, &corrupt),
        Err(ProfileStoreError::Corrupt(_))
    ));
    assert!(!inherited.contains_agent(&revision.content.agent_profiles[0]));
    inherited.inherit_workspace(&parent, &revision).unwrap();
    assert!(inherited.contains_agent(&revision.content.agent_profiles[0]));
    assert!(!inherited.contains_agent(&corrupt.content.agent_profiles[0]));
}

async fn visibility(
    store: &InMemoryProfileStore,
    owner: &SubjectId,
    locator: MarketplaceLocator,
    visibility: ProfileVisibility,
) {
    store
        .set_profile_visibility(owner, &locator, visibility, NOW.into(), bounds(), &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn forked_workspace_preserves_exact_private_and_retired_agent_pins() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_inherited", "publisher", ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    for _ in 0..2 {
        store
            .publish_agent_profile(&agent.id, 1, NOW.into())
            .await
            .unwrap();
    }
    let workspace = workspace_profile(
        "wkp_source",
        "publisher",
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
    visibility(
        &store,
        &workspace.owner,
        MarketplaceLocator::Workspace(workspace.id.clone()),
        ProfileVisibility::Public,
    )
    .await;
    let consumer = SubjectId("consumer".into());
    let ForkedProfile::Workspace(mut fork) = store
        .fork_profile(
            &consumer,
            &MarketplaceLocator::Workspace(workspace.id.clone()),
            1,
            NOW.into(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    // The agent was never independently public: the source revision is the grant.
    assert!(store
        .read_marketplace(&MarketplaceLocator::Agent(agent.id.clone()))
        .await
        .is_err());
    fork.draft_version = 2;
    fork.description = "Edited by the second owner".into();
    references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.agent_profiles,
    )
    .await
    .unwrap();
    store.save_workspace_draft(1, fork.clone()).await.unwrap();
    store
        .publish_workspace_profile(&fork.id, 2, NOW.into())
        .await
        .unwrap();
    visibility(
        &store,
        &workspace.owner,
        MarketplaceLocator::Workspace(workspace.id.clone()),
        ProfileVisibility::Private,
    )
    .await;
    store
        .retire_agent_profile(&agent.id, &agent.owner, NOW.into())
        .await
        .unwrap();
    store
        .retire_workspace_profile(&workspace.id, &workspace.owner, NOW.into())
        .await
        .unwrap();
    references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.agent_profiles,
    )
    .await
    .unwrap();
    store
        .publish_workspace_profile(&fork.id, 2, NOW.into())
        .await
        .unwrap();
    fork.draft.agent_profiles[0].revision = 2;
    fork.draft_version = 3;
    assert!(references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.agent_profiles
    )
    .await
    .is_err());
    store.save_workspace_draft(2, fork.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&fork.id, 3, NOW.into())
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    assert_eq!(
        store
            .list_workspace_profile_revisions(&fork.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
}

#[tokio::test]
async fn forked_sandbox_preserves_exact_private_and_retired_workspace_pins() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_nested", "publisher", ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();
    let workspace = workspace_profile("wkp_nested", "publisher", AgentProfileRef::new(agent.id, 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .publish_workspace_profile(&workspace.id, 1, NOW.into())
            .await
            .unwrap();
    }
    let sandbox = sandbox_profile(
        "sbp_source",
        "publisher",
        WorkspaceProfileRef::new(workspace.id.clone(), 1),
    );
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    let consumer = SubjectId("consumer".into());
    let ForkedProfile::Sandbox(mut fork) = store
        .fork_profile(
            &consumer,
            &MarketplaceLocator::Sandbox(sandbox.id.clone()),
            1,
            NOW.into(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    fork.draft_version = 2;
    fork.description = "Edited by the second owner".into();
    references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.workspace_profiles,
    )
    .await
    .unwrap();
    store.save_sandbox_draft(1, fork.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&fork.id, 2, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    visibility(
        &store,
        &sandbox.owner,
        MarketplaceLocator::Sandbox(sandbox.id.clone()),
        ProfileVisibility::Private,
    )
    .await;
    store
        .retire_workspace_profile(&workspace.id, &workspace.owner, NOW.into())
        .await
        .unwrap();
    store
        .retire_sandbox_profile(&sandbox.id, &sandbox.owner, NOW.into())
        .await
        .unwrap();
    references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.workspace_profiles,
    )
    .await
    .unwrap();
    store
        .publish_sandbox_profile(&fork.id, 2, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    fork.draft.workspace_profiles[0].revision = 2;
    fork.draft_version = 3;
    assert!(references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&fork.id),
        &fork.draft.workspace_profiles
    )
    .await
    .is_err());
    store.save_sandbox_draft(2, fork.clone()).await.unwrap();
    assert_eq!(
        store
            .publish_sandbox_profile(&fork.id, 3, NOW.into(), bounds(), &[])
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
}

#[tokio::test]
async fn prior_publications_preserve_pins_but_arbitrary_drafts_and_forged_provenance_do_not() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_prior", "publisher", ProfileVisibility::Public);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();
    let workspace = workspace_profile(
        "wkp_prior",
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
    visibility(
        &store,
        &agent.owner,
        MarketplaceLocator::Agent(agent.id.clone()),
        ProfileVisibility::Private,
    )
    .await;
    store
        .retire_agent_profile(&agent.id, &agent.owner, NOW.into())
        .await
        .unwrap();
    references::validate_agent_references(
        &store,
        &store,
        &workspace.owner,
        Some(&workspace.id),
        &workspace.draft.agent_profiles,
    )
    .await
    .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();
    let mut arbitrary = workspace.clone();
    arbitrary.id = WorkspaceProfileId("wkp_arbitrary".into());
    store
        .create_workspace_profile(arbitrary.clone())
        .await
        .unwrap();
    assert!(references::validate_agent_references(
        &store,
        &store,
        &arbitrary.owner,
        Some(&arbitrary.id),
        &arbitrary.draft.agent_profiles
    )
    .await
    .is_err());
    assert_eq!(
        store
            .publish_workspace_profile(&arbitrary.id, 1, NOW.into())
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    arbitrary.forked_from = Some(WorkspaceProfileRef::new(workspace.id, 1));
    arbitrary.draft_version = 2;
    assert_eq!(
        store
            .save_workspace_draft(1, arbitrary.clone())
            .await
            .unwrap_err(),
        ProfileStoreError::Conflict
    );
    arbitrary.id = WorkspaceProfileId("wkp_forged".into());
    arbitrary.draft_version = 1;
    assert_eq!(
        store.create_workspace_profile(arbitrary).await.unwrap_err(),
        ProfileStoreError::Conflict
    );
    let mut forged_agent = agent.clone();
    forged_agent.id = AgentProfileId("agp_forged".into());
    forged_agent.forked_from = Some(AgentProfileRef::new(agent.id, 1));
    assert_eq!(
        store.create_agent_profile(forged_agent).await.unwrap_err(),
        ProfileStoreError::Conflict
    );
    let mut forged_sandbox = sandbox_profile(
        "sbp_forged",
        "consumer",
        WorkspaceProfileRef::new(WorkspaceProfileId("wkp_prior".into()), 1),
    );
    forged_sandbox.forked_from = Some(SandboxProfileRef::new(
        SandboxProfileId("sbp_hidden".into()),
        1,
    ));
    assert_eq!(
        store
            .create_sandbox_profile(forged_sandbox)
            .await
            .unwrap_err(),
        ProfileStoreError::Conflict
    );
}

#[tokio::test]
async fn prior_sandbox_publications_preserve_exact_workspace_pins() {
    let store = InMemoryProfileStore::default();
    let agent = agent_profile("agp_sandbox_prior", "publisher", ProfileVisibility::Private);
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, NOW.into())
        .await
        .unwrap();
    let workspace = workspace_profile(
        "wkp_sandbox_prior",
        "publisher",
        AgentProfileRef::new(agent.id, 1),
    );
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, NOW.into())
        .await
        .unwrap();
    visibility(
        &store,
        &workspace.owner,
        MarketplaceLocator::Workspace(workspace.id.clone()),
        ProfileVisibility::Public,
    )
    .await;
    let sandbox = sandbox_profile(
        "sbp_prior",
        "consumer",
        WorkspaceProfileRef::new(workspace.id.clone(), 1),
    );
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    visibility(
        &store,
        &workspace.owner,
        MarketplaceLocator::Workspace(workspace.id.clone()),
        ProfileVisibility::Private,
    )
    .await;
    store
        .retire_workspace_profile(&workspace.id, &workspace.owner, NOW.into())
        .await
        .unwrap();
    references::validate_workspace_references(
        &store,
        &store,
        &sandbox.owner,
        Some(&sandbox.id),
        &sandbox.draft.workspace_profiles,
    )
    .await
    .unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, NOW.into(), bounds(), &[])
        .await
        .unwrap();
    let mut arbitrary = sandbox;
    arbitrary.id = SandboxProfileId("sbp_arbitrary".into());
    store
        .create_sandbox_profile(arbitrary.clone())
        .await
        .unwrap();
    assert!(references::validate_workspace_references(
        &store,
        &store,
        &arbitrary.owner,
        Some(&arbitrary.id),
        &arbitrary.draft.workspace_profiles
    )
    .await
    .is_err());
    assert_eq!(
        store
            .publish_sandbox_profile(&arbitrary.id, 1, NOW.into(), bounds(), &[])
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
}
