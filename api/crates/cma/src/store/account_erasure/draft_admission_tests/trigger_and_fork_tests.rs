use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_draft_create_rejects_fenced_profile_and_leaves_no_definition() {
    let seed = seeded("erasure_trigger_draft_create").await;
    let db = seed.client.database(&seed.database);
    let owner = SubjectId::new("survivor");
    seed.store
        .create_trigger(trigger("trg_before_fence", &owner, &seed.target))
        .await
        .unwrap();
    for (collection, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        fence(&db, collection, id, true).await;
        let id = TriggerId(format!("trg_fenced_{collection}"));
        assert!(matches!(
            seed.store
                .create_trigger(trigger(id.as_str(), &owner, &seed.target))
                .await,
            Err(TriggerStoreError::Reference(
                TriggerReferenceError::Unavailable { .. }
            ))
        ));
        assert!(seed.store.trigger(&id, &owner).await.unwrap().is_none());
        fence(
            &db,
            collection,
            match collection {
                "sandbox_profiles" => seed.target.sandbox_profile.id.as_str(),
                "workspace_profiles" => seed.target.workspace_profile.id.as_str(),
                _ => seed.target.agent_profile.id.as_str(),
            },
            false,
        )
        .await;
    }
    seed.store
        .create_trigger(trigger("trg_after_fence", &owner, &seed.target))
        .await
        .unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn trigger_draft_save_rejects_new_fenced_profile_and_preserves_original_content() {
    let seed = seeded("erasure_trigger_draft_save").await;
    let db = seed.client.database(&seed.database);
    let owner = SubjectId::new("survivor");
    let id = TriggerId("trg_survivor_draft".into());
    let baseline = trigger(id.as_str(), &owner, &seed.target);
    seed.store.create_trigger(baseline.clone()).await.unwrap();
    let mut added = seed
        .profiles
        .agent_profile(&seed.target.agent_profile.id)
        .await
        .unwrap()
        .unwrap();
    added.id = AgentProfileId("agp_new_draft_target".into());
    seed.profiles
        .create_agent_profile(added.clone())
        .await
        .unwrap();
    seed.profiles
        .publish_agent_profile(&added.id, 1, NOW.into())
        .await
        .unwrap();
    let mut changed = baseline.draft.clone();
    changed.target.agent_profile.id = added.id.clone();
    fence(&db, "agent_profiles", added.id.as_str(), true).await;
    assert!(matches!(
        seed.store
            .save_draft(&id, &owner, 1, changed.clone(), NOW.into())
            .await,
        Err(TriggerStoreError::Reference(
            TriggerReferenceError::Unavailable { .. }
        ))
    ));
    assert_eq!(
        seed.store.trigger(&id, &owner).await.unwrap(),
        Some(baseline)
    );
    fence(&db, "agent_profiles", added.id.as_str(), false).await;
    let saved = seed
        .store
        .save_draft(&id, &owner, 1, changed.clone(), NOW.into())
        .await
        .unwrap();
    assert_eq!(saved.draft, changed);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn marketplace_fork_rejects_fenced_source_and_keeps_existing_forks() {
    let seed = seeded("erasure_marketplace_fork_source").await;
    let db = seed.client.database(&seed.database);
    let owner = SubjectId::new("survivor");
    for (collection, locator) in [
        (
            "agent_profiles",
            MarketplaceLocator::Agent(seed.target.agent_profile.id.clone()),
        ),
        (
            "workspace_profiles",
            MarketplaceLocator::Workspace(seed.target.workspace_profile.id.clone()),
        ),
        (
            "sandbox_profiles",
            MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
        ),
    ] {
        seed.profiles
            .fork_profile(&owner, &locator, 1, NOW.into())
            .await
            .unwrap();
        let before = db
            .collection::<Document>(collection)
            .count_documents(doc! {"owner":"survivor"})
            .await
            .unwrap();
        fence(&db, collection, locator.as_str(), true).await;
        assert!(matches!(
            seed.profiles
                .fork_profile(&owner, &locator, 1, NOW.into())
                .await,
            Err(MarketplaceError::NotFound)
        ));
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {"owner":"survivor"})
                .await
                .unwrap(),
            before
        );
        fence(&db, collection, locator.as_str(), false).await;
        seed.profiles
            .fork_profile(&owner, &locator, 1, NOW.into())
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn marketplace_fork_rejects_a_fenced_transitive_child_without_creating_a_root() {
    let seed = seeded("erasure_marketplace_fork_child").await;
    let db = seed.client.database(&seed.database);
    let owner = SubjectId::new("survivor");
    for locator in [
        MarketplaceLocator::Workspace(seed.target.workspace_profile.id.clone()),
        MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
    ] {
        fence(
            &db,
            "agent_profiles",
            seed.target.agent_profile.id.as_str(),
            true,
        )
        .await;
        assert!(matches!(
            seed.profiles
                .fork_profile(&owner, &locator, 1, NOW.into())
                .await,
            Err(MarketplaceError::NotFound)
        ));
        fence(
            &db,
            "agent_profiles",
            seed.target.agent_profile.id.as_str(),
            false,
        )
        .await;
        seed.profiles
            .fork_profile(&owner, &locator, 1, NOW.into())
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}
