use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_organization_inventory_is_classified_before_personal_capacity() {
    let db = database().await;
    sandbox(&db, "organization", "organization-root").await;
    let inventory = ErasureInventory {
        agents: (0..20_001).map(|n| format!("org-{n:05}")).collect(),
        ..Default::default()
    };
    db.collection::<Document>("agents")
        .insert_many(inventory.agents.iter().map(|id| {
            doc! {"_id":id,"owner":"organization","sandbox_id":"organization-root"}
        }))
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let loaded = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let mut merged = context::Context::default();
    merged.subject = "a".into();
    assert!(!merged.merge_inventory(&db, &inventory).await.unwrap());
    // Both initial discovery and later durable-inventory unions classify first.
    for context in loaded.into_iter().chain(std::iter::once(merged)) {
        assert!(context.ids(catalog::InventorySet::Agents).is_empty());
        assert!(context.protected.contains("org-20000"));
        let mut cursor = Cursor {
            subjects: BTreeSet::from(["a".into()]),
            contexts: vec![context],
            ..Default::default()
        };
        progress::cursor_capacity(&mut cursor).unwrap();
        assert_eq!(cursor.contexts.len(), 1);
    }
    assert_eq!(count(&db, "agents").await, 20_001);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mixed_inventory_excludes_foreign_ids_without_weakening_personal_limits() {
    let db = database().await;
    for id in ["foreign-a", "foreign-b"] {
        put(&db, "agents", doc! {"_id":id,"owner":"organization"}).await;
    }
    let mut inventory = ErasureInventory {
        agents: (0..19_999).map(|n| format!("own-{n:05}")).collect(),
        ..Default::default()
    };
    inventory
        .agents
        .extend(["foreign-a".into(), "foreign-b".into()]);
    let subject = SubjectId::new("a");
    let mut loaded = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap()
    .pop()
    .unwrap();
    let mut merged = context::Context::default();
    merged.subject = "a".into();
    assert!(merged.merge_inventory(&db, &inventory).await.unwrap());
    for context in [&mut loaded, &mut merged] {
        assert_eq!(context.ids(catalog::InventorySet::Agents).len(), 19_999);
        assert!(context.protected.contains("foreign-a"));
        let mut boundary = inventory.clone();
        boundary.agents.push("own-boundary".into());
        assert!(context.merge_inventory(&db, &boundary).await.unwrap());
        assert_eq!(context.ids(catalog::InventorySet::Agents).len(), 20_000);
        boundary.agents.push("own-overflow".into());
        assert_capacity(
            context.merge_inventory(&db, &boundary).await.unwrap_err(),
            "a",
            "personal resource IDs",
            20_000,
        );
    }
    assert_eq!(count(&db, "agents").await, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn inventory_classification_honors_parent_and_rootless_sidecar_ownership() {
    let db = database().await;
    put(
        &db,
        "agents",
        doc! {"_id":"current-agent","owner":"a","sandbox_id":"organization-root"},
    )
    .await;
    put(
        &db,
        "resource_policies",
        doc! {"_id":"organization-root","owner_kind":"organization","owner_subject":"organization"},
    )
    .await;
    put(&db, "agent_creation_provenance", doc! {"_id":"purged-agent","record":{
        "owner":{"kind":"organization","subject":"organization"},"sandbox_id":"organization-root",
    }}).await;
    put(
        &db,
        "trigger_launch_input_protocols",
        doc! {"_id":"purged-launch","owner":"b"},
    )
    .await;
    put(
        &db,
        "binding_conversations",
        doc! {"_id":"conversation","binding_id":"purged-binding",
        "record":{"owner":"b","binding_id":"purged-binding","agent_id":"binding-agent"}},
    )
    .await;
    let inventory = ErasureInventory {
        agents: vec![
            "current-agent".into(),
            "purged-agent".into(),
            "personal-orphan".into(),
        ],
        launches: vec!["purged-launch".into()],
        bindings: vec!["purged-binding".into()],
        ..Default::default()
    };
    let subject = SubjectId::new("a");
    let loaded = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let mut merged = context::Context::default();
    merged.subject = "a".into();
    assert!(merged.merge_inventory(&db, &inventory).await.unwrap());
    for mut context in loaded.into_iter().chain(std::iter::once(merged)) {
        assert_eq!(
            context.ids(catalog::InventorySet::Agents),
            ["personal-orphan"]
        );
        assert!(context.ids(catalog::InventorySet::Launches).is_empty());
        assert!(context.ids(catalog::InventorySet::Bindings).is_empty());
        for id in [
            "current-agent",
            "purged-agent",
            "purged-launch",
            "purged-binding",
            "binding-agent",
        ] {
            assert!(context.protected.contains(id), "{id}");
        }
        assert!(context.organization_sandboxes.contains("organization-root"));
        assert!(!context.merge_inventory(&db, &inventory).await.unwrap());
    }
    assert_eq!(count(&db, "agents").await, 1);
    assert_eq!(count(&db, "agent_creation_provenance").await, 1);
    assert_eq!(count(&db, "trigger_launch_input_protocols").await, 1);
    assert_eq!(count(&db, "binding_conversations").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_inventory_identity_is_a_typed_block_for_its_subject() {
    let db = database().await;
    for id in [String::new(), "x".repeat(4097)] {
        let inventory = ErasureInventory {
            agents: vec![id],
            ..Default::default()
        };
        let mut context = context::Context::default();
        context.subject = "b".into();
        let error = context.merge_inventory(&db, &inventory).await.unwrap_err();
        let Error::Blocked(block) = error else {
            panic!("invalid inventory must be an attributed block, got {error:?}");
        };
        assert_eq!(block.subject_hash, subject_hash(&SubjectId::new("b")));
        assert_eq!(
            block.reason,
            ErasureBlock::CorruptRecord {
                collection: "historical resource identity".into(),
            }
        );
        assert!(context.sets.is_empty());
    }
    db.drop().await.unwrap();
}
