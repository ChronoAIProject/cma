use super::*;
use layer2_core::profiles::references;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupt_inherited_evidence_and_child_digests_refuse_atomic_publication() {
    let (client, name, store) = database("invalid_refs").await;
    let agent = agent_profile("agp_digest", "publisher", "Private child");
    store.create_agent_profile(agent.clone()).await.unwrap();
    let child = store
        .publish_agent_profile(&agent.id, 1, T1.into())
        .await
        .unwrap();
    let workspace = workspace_profile("wkp_digest", AgentProfileRef::new(agent.id.clone(), 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    let source = store
        .publish_workspace_profile(&workspace.id, 1, T1.into())
        .await
        .unwrap();
    make_public(
        &store,
        &workspace.owner,
        &MarketplaceLocator::Workspace(workspace.id.clone()),
    )
    .await;
    let consumer = SubjectId("consumer".into());
    let ForkedProfile::Workspace(fork) = store
        .fork_profile(
            &consumer,
            &MarketplaceLocator::Workspace(workspace.id.clone()),
            1,
            T2.into(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    store
        .workspace_revisions
        .update_one(
            doc! { "profile_id": workspace.id.as_str(), "revision": 1_i64 },
            doc! { "$set": { "record.digest": "corrupt-digest" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .publish_workspace_profile(&fork.id, 1, T2.into())
            .await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    store
        .workspace_revisions
        .update_one(
            doc! { "profile_id": workspace.id.as_str(), "revision": 1_i64 },
            doc! { "$set": { "record.digest": &source.digest } },
        )
        .await
        .unwrap();
    store
        .agent_revisions
        .update_one(
            doc! { "profile_id": agent.id.as_str(), "revision": 1_i64 },
            doc! { "$set": { "record.digest": "corrupt-digest" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .publish_workspace_profile(&fork.id, 1, T2.into())
            .await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    assert!(store
        .list_workspace_profile_revisions(&fork.id, &ProfilePageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    store
        .agent_revisions
        .update_one(
            doc! { "profile_id": agent.id.as_str(), "revision": 1_i64 },
            doc! { "$set": { "record.digest": &child.digest } },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&fork.id, 1, T2.into())
            .await
            .unwrap()
            .revision,
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn exact_fork_pins_survive_private_and_retired_children_in_mongo() {
    let (client, name, store) = database("inherited_refs").await;
    let agent = agent_profile("agp_inherited", "publisher", "Private child");
    store.create_agent_profile(agent.clone()).await.unwrap();
    for _ in 0..2 {
        store
            .publish_agent_profile(&agent.id, 1, T1.into())
            .await
            .unwrap();
    }
    let workspace = workspace_profile("wkp_inherited", AgentProfileRef::new(agent.id.clone(), 1));
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .publish_workspace_profile(&workspace.id, 1, T1.into())
            .await
            .unwrap();
    }
    let sandbox = sandbox_profile(
        "sbp_inherited",
        vec![WorkspaceProfileRef::new(workspace.id.clone(), 1)],
    );
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, T1.into(), bounds(), &[])
        .await
        .unwrap();
    make_public(
        &store,
        &sandbox.owner,
        &MarketplaceLocator::Sandbox(sandbox.id.clone()),
    )
    .await;
    let consumer = SubjectId("consumer".into());
    // Fork the sandbox while its referenced workspace has never been public.
    let ForkedProfile::Sandbox(mut sandbox_fork) = store
        .fork_profile(
            &consumer,
            &MarketplaceLocator::Sandbox(sandbox.id.clone()),
            1,
            T2.into(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    make_public(
        &store,
        &workspace.owner,
        &MarketplaceLocator::Workspace(workspace.id.clone()),
    )
    .await;
    // Its referenced agent has never been public either.
    let ForkedProfile::Workspace(mut workspace_fork) = store
        .fork_profile(
            &consumer,
            &MarketplaceLocator::Workspace(workspace.id.clone()),
            1,
            T2.into(),
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    workspace_fork.draft_version = 2;
    workspace_fork.description = "Second owner workspace edit".into();
    sandbox_fork.draft_version = 2;
    sandbox_fork.description = "Second owner sandbox edit".into();
    references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&workspace_fork.id),
        &workspace_fork.draft.agent_profiles,
    )
    .await
    .unwrap();
    references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&sandbox_fork.id),
        &sandbox_fork.draft.workspace_profiles,
    )
    .await
    .unwrap();
    store
        .save_workspace_draft(1, workspace_fork.clone())
        .await
        .unwrap();
    store
        .save_sandbox_draft(1, sandbox_fork.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace_fork.id, 2, T2.into())
        .await
        .unwrap();
    store
        .publish_sandbox_profile(&sandbox_fork.id, 2, T2.into(), bounds(), &[])
        .await
        .unwrap();
    for locator in [
        MarketplaceLocator::Workspace(workspace.id.clone()),
        MarketplaceLocator::Sandbox(sandbox.id.clone()),
    ] {
        store
            .set_profile_visibility(
                &agent.owner,
                &locator,
                ProfileVisibility::Private,
                T3.into(),
                bounds(),
                &[],
            )
            .await
            .unwrap();
    }
    store
        .retire_agent_profile(&agent.id, &agent.owner, T3.into())
        .await
        .unwrap();
    store
        .retire_workspace_profile(&workspace.id, &workspace.owner, T3.into())
        .await
        .unwrap();
    store
        .retire_sandbox_profile(&sandbox.id, &sandbox.owner, T3.into())
        .await
        .unwrap();
    // Reopen persistence before exercising the protected grants again.
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&workspace_fork.id),
        &workspace_fork.draft.agent_profiles,
    )
    .await
    .unwrap();
    references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&sandbox_fork.id),
        &sandbox_fork.draft.workspace_profiles,
    )
    .await
    .unwrap();
    store
        .publish_workspace_profile(&workspace_fork.id, 2, T3.into())
        .await
        .unwrap();
    store
        .publish_sandbox_profile(&sandbox_fork.id, 2, T3.into(), bounds(), &[])
        .await
        .unwrap();
    workspace_fork.draft.agent_profiles[0].revision = 2;
    workspace_fork.draft_version = 3;
    sandbox_fork.draft.workspace_profiles[0].revision = 2;
    sandbox_fork.draft_version = 3;
    assert!(references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&workspace_fork.id),
        &workspace_fork.draft.agent_profiles
    )
    .await
    .is_err());
    assert!(references::validate_workspace_references(
        &store,
        &store,
        &consumer,
        Some(&sandbox_fork.id),
        &sandbox_fork.draft.workspace_profiles
    )
    .await
    .is_err());
    store
        .save_workspace_draft(2, workspace_fork.clone())
        .await
        .unwrap();
    store
        .save_sandbox_draft(2, sandbox_fork.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&workspace_fork.id, 3, T4.into())
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    assert_eq!(
        store
            .publish_sandbox_profile(&sandbox_fork.id, 3, T4.into(), bounds(), &[])
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    assert_eq!(
        store
            .list_workspace_profile_revisions(&workspace_fork.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store
            .list_sandbox_profile_revisions(&sandbox_fork.id, &ProfilePageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(store
        .read_marketplace(&MarketplaceLocator::Agent(agent.id))
        .await
        .is_err());
    assert!(store
        .read_marketplace(&MarketplaceLocator::Workspace(workspace.id))
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn prior_published_pins_and_forged_provenance_have_mongo_parity() {
    let (client, name, store) = database("prior_refs").await;
    let agent = agent_profile("agp_prior", "publisher", "Prior child");
    store.create_agent_profile(agent.clone()).await.unwrap();
    store
        .publish_agent_profile(&agent.id, 1, T1.into())
        .await
        .unwrap();
    make_public(
        &store,
        &agent.owner,
        &MarketplaceLocator::Agent(agent.id.clone()),
    )
    .await;
    let consumer = SubjectId("consumer".into());
    let mut workspace = workspace_profile("wkp_prior", AgentProfileRef::new(agent.id.clone(), 1));
    workspace.owner = consumer.clone();
    store
        .create_workspace_profile(workspace.clone())
        .await
        .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, T1.into())
        .await
        .unwrap();
    make_public(
        &store,
        &workspace.owner,
        &MarketplaceLocator::Workspace(workspace.id.clone()),
    )
    .await;
    let sandbox = sandbox_profile(
        "sbp_prior",
        vec![WorkspaceProfileRef::new(workspace.id.clone(), 1)],
    );
    store.create_sandbox_profile(sandbox.clone()).await.unwrap();
    store
        .publish_sandbox_profile(&sandbox.id, 1, T1.into(), bounds(), &[])
        .await
        .unwrap();
    store
        .set_profile_visibility(
            &agent.owner,
            &MarketplaceLocator::Agent(agent.id.clone()),
            ProfileVisibility::Private,
            T2.into(),
            bounds(),
            &[],
        )
        .await
        .unwrap();
    store
        .retire_agent_profile(&agent.id, &agent.owner, T2.into())
        .await
        .unwrap();
    references::validate_agent_references(
        &store,
        &store,
        &consumer,
        Some(&workspace.id),
        &workspace.draft.agent_profiles,
    )
    .await
    .unwrap();
    store
        .publish_workspace_profile(&workspace.id, 1, T2.into())
        .await
        .unwrap();
    store
        .set_profile_visibility(
            &workspace.owner,
            &MarketplaceLocator::Workspace(workspace.id.clone()),
            ProfileVisibility::Private,
            T2.into(),
            bounds(),
            &[],
        )
        .await
        .unwrap();
    store
        .retire_workspace_profile(&workspace.id, &workspace.owner, T2.into())
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
        .publish_sandbox_profile(&sandbox.id, 1, T2.into(), bounds(), &[])
        .await
        .unwrap();
    let mut arbitrary = workspace.clone();
    arbitrary.id = WorkspaceProfileId("wkp_arbitrary".into());
    store
        .create_workspace_profile(arbitrary.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_workspace_profile(&arbitrary.id, 1, T3.into())
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
    let mut arbitrary_sandbox = sandbox;
    arbitrary_sandbox.id = SandboxProfileId("sbp_arbitrary".into());
    store
        .create_sandbox_profile(arbitrary_sandbox.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .publish_sandbox_profile(&arbitrary_sandbox.id, 1, T3.into(), bounds(), &[])
            .await
            .unwrap_err(),
        ProfileStoreError::ReferenceUnavailable
    );
    arbitrary_sandbox.forked_from = Some(layer2_core::profiles::SandboxProfileRef::new(
        SandboxProfileId("sbp_prior".into()),
        1,
    ));
    arbitrary_sandbox.draft_version = 2;
    assert_eq!(
        store
            .save_sandbox_draft(1, arbitrary_sandbox.clone())
            .await
            .unwrap_err(),
        ProfileStoreError::Conflict
    );
    arbitrary_sandbox.id = SandboxProfileId("sbp_forged".into());
    arbitrary_sandbox.draft_version = 1;
    assert_eq!(
        store
            .create_sandbox_profile(arbitrary_sandbox)
            .await
            .unwrap_err(),
        ProfileStoreError::Conflict
    );
    client.database(&name).drop().await.unwrap();
}
