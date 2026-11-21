use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_graph_capacity_is_charged_to_its_owner_instead_of_a_larger_resource_context() {
    let seed = seeded("round3_graph_attribution").await;
    let db = seed.client.database(&seed.database);
    let profiles = db.collection::<Document>("agent_profiles");
    profiles.insert_many((0..400).map(|n| doc! {"_id":format!("agp_large_{n:04}_{}","x".repeat(1000)),"owner":seed.owner.as_str(),"record":{}})).await.unwrap();
    let a = SubjectId::new("a");
    let large_inventory = ErasureInventory {
        agents: (0..11_000)
            .map(|n| format!("own-{n:05}-{}", "z".repeat(30)))
            .collect(),
        ..Default::default()
    };
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let mut blocked = false;
    for _ in 0..100 {
        let result = erase_records(
            &db,
            &[
                Target {
                    subject: &a,
                    inventory: &large_inventory,
                },
                Target {
                    subject: &seed.owner,
                    inventory: &inventory,
                },
            ],
            &mut cursor,
            NOW,
        )
        .await;
        match result {
            Err(Error::Blocked(AttributedBlock {
                subject_hash,
                reason: ErasureBlock::CapacityExceeded { class, .. },
            })) => {
                assert!(
                    matches!(
                        class.as_str(),
                        "serialized member state bytes" | "serialized cohort cursor bytes"
                    ),
                    "the fixture must reach the shared serialized-state budget: {class}"
                );
                assert_eq!(
                    subject_hash,
                    layer2_core::account_deletion::subject_hash(&seed.owner)
                );
                blocked = true;
                break;
            }
            Ok(batch) => assert!(batch.deleted.is_empty() && batch.redacted.is_empty()),
            other => panic!("unexpected discovery result: {other:?}"),
        }
    }
    assert!(
        blocked,
        "Profile graph bytes require a per-owner typed limit before effects"
    );
    assert!(serde_json::to_vec(&cursor).unwrap().len() <= progress::MAX_CURSOR_BYTES);
    assert_eq!(profiles.count_documents(doc! {}).await.unwrap(), 401);
    profiles
        .delete_many(doc! {"_id":{"$regex":"^agp_large_"}})
        .await
        .unwrap();
    let retained = profile_refs::retained(&db, &BTreeSet::from([seed.owner.to_string()]))
        .await
        .unwrap();
    assert!(retained.is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn shared_revision_page_charges_large_growth_instead_of_its_last_small_author() {
    let db = database().await;
    let a = SubjectId::new("a");
    let b = SubjectId::new("b");
    let large = format!("agp_a_{}", "x".repeat(990));
    for (id, owner) in [(&large[..], "a"), ("agp_z_small", "b")] {
        put(
            &db,
            "agent_profiles",
            doc! {"_id":id,"owner":owner,"record":{}},
        )
        .await;
    }
    let revisions = db.collection::<Document>("agent_profile_revisions");
    revisions.insert_many((1..=200).map(|revision| doc! {
        "_id":format!("{large}:{revision:010}"),"profile_id":&large,"revision":revision,
        "document_schema_version":1_i64,"record":{"profile_id":&large,"revision":revision,"content":{}}
    })).await.unwrap();
    put(&db, "agent_profile_revisions", doc! {
        "_id":"agp_z_small:0000000001","profile_id":"agp_z_small","revision":1_i64,
        "document_schema_version":1_i64,"record":{"profile_id":"agp_z_small","revision":1_i64,"content":{}}
    }).await;
    let inventory = |owner: &str| ErasureInventory {
        agents: (0..8000)
            .map(|n| format!("{owner}-{n:05}-{}", "x".repeat(25)))
            .collect(),
        ..Default::default()
    };
    let ai = inventory("a");
    let bi = inventory("b");
    let targets = [
        Target {
            subject: &a,
            inventory: &ai,
        },
        Target {
            subject: &b,
            inventory: &bi,
        },
    ];
    let mut cursor = Cursor::default();
    let mut refused = false;
    for _ in 0..100 {
        match erase_records(&db, &targets, &mut cursor, NOW).await {
            Err(Error::Blocked(AttributedBlock {
                subject_hash,
                reason: ErasureBlock::CapacityExceeded { class, .. },
            })) => {
                assert_eq!(
                    class, "serialized cohort cursor bytes",
                    "each member must fit its individual budget"
                );
                assert_eq!(
                    subject_hash,
                    layer2_core::account_deletion::subject_hash(&a),
                    "the tiny final author must not inherit the larger author's page growth"
                );
                refused = true;
                break;
            }
            Ok(batch) => assert!(batch.deleted.is_empty() && batch.redacted.is_empty()),
            other => panic!("unexpected discovery outcome: {other:?}"),
        }
    }
    assert!(
        refused,
        "the shared revision page must reach the cohort budget"
    );
    assert_eq!(revisions.count_documents(doc! {}).await.unwrap(), 201);
    revisions
        .delete_many(doc! {"profile_id":&large})
        .await
        .unwrap();
    let mut cursor = Cursor::default();
    for _ in 0..100 {
        let batch = erase_records(&db, &targets, &mut cursor, NOW)
            .await
            .unwrap();
        assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
        if cursor.retained_profiles.is_some() {
            break;
        }
    }
    assert!(
        cursor.retained_profiles.is_some(),
        "removing the oversized contribution permits discovery"
    );
    db.drop().await.unwrap();
}
