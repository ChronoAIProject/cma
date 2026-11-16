//! Capacity rejection follows the actual section and never grows skip lists.
use super::{tests::*, *};
use crate::store::trigger_definitions::integration_tests::seeded;
use mongodb::bson::{doc, Bson, Document};

async fn malformed_sandbox_owner(update: Document) {
    let db = database().await;
    sandbox(&db, "a", "malformed-root").await;
    put(
        &db,
        "agents",
        doc! {"_id":"owned-agent","owner":"a","sandbox_id":"malformed-root"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":"owned-log","agent_id":"owned-agent"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":"foreign-log","agent_id":"foreign-agent"},
    )
    .await;
    db.collection::<Document>("sandboxes")
        .update_one(doc! {"_id":"malformed-root"}, update)
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let mut inventory = ErasureInventory {
        sandboxes: vec!["malformed-root".into()],
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let mut cursor = Cursor::default();
    let mut refused = false;
    for _ in 0..100 {
        match erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        {
            Err(Error::Blocked(AttributedBlock {
                subject_hash,
                reason: ErasureBlock::CorruptRecord { collection },
            })) => {
                assert_eq!(
                    subject_hash,
                    layer2_core::account_deletion::subject_hash(&subject)
                );
                assert_eq!(collection, "sandboxes");
                refused = true;
                break;
            }
            Ok(batch) => {
                assert!(
                    batch.deleted.is_empty() && batch.redacted.is_empty(),
                    "a malformed personal root cannot authorize descendant deletion"
                );
                progress::acknowledge(&mut inventory, &batch, subject.as_str());
            }
            other => panic!("unexpected malformed-root result: {other:?}"),
        }
    }
    assert!(
        refused,
        "invalid query-field ownership must block before effects"
    );
    assert_eq!(count(&db, "agents").await, 1);
    assert_eq!(count(&db, "agent_log").await, 2);
    db.collection::<Document>("sandboxes")
        .update_one(doc! {"_id":"malformed-root"}, doc! {"$set":{"owner":"a"}})
        .await
        .unwrap();
    drain(&db, "a", &mut inventory).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    assert_eq!(count(&db, "agents").await, 0);
    assert_eq!(count(&db, "agent_log").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_personal_sandbox_missing_owner_blocks_before_descendant_effects() {
    malformed_sandbox_owner(doc! {"$unset":{"owner":""}}).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_personal_sandbox_nonstring_owner_blocks_before_descendant_effects() {
    malformed_sandbox_owner(doc! {"$set":{"owner":17_i64}}).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn single_person_snapshot_budget_leaves_room_for_ids_and_discovery() {
    let db = database().await;
    sandbox(&db, "a", "snapshot-root").await;
    let collection = db.collection::<Document>("sandboxes");
    collection
        .update_one(
            doc! {"_id":"snapshot-root"},
            doc! {"$set":{"legacy_padding":"x".repeat(256 * 1024)}},
        )
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let result = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await;
    assert!(
        matches!(result, Err(Error::Blocked(AttributedBlock {subject_hash, reason:ErasureBlock::CapacityExceeded {..}})) if subject_hash == layer2_core::account_deletion::subject_hash(&subject)),
        "live snapshots must leave space for bounded resource and Profile state"
    );
    collection
        .update_one(
            doc! {"_id":"snapshot-root"},
            doc! {"$unset":{"legacy_padding":""}},
        )
        .await
        .unwrap();
    assert!(context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory
        }]
    )
    .await
    .is_ok());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn large_organization_snapshot_does_not_spend_a_personal_snapshot_budget() {
    use layer2_core::resource_access::{
        OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
    };
    let db = database().await;
    sandbox(&db, "organization", "organization-root").await;
    let collection = db.collection::<Document>("sandboxes");
    collection
        .update_one(
            doc! {"_id":"organization-root"},
            doc! {"$set":{"legacy_padding":"x".repeat(256 * 1024)}},
        )
        .await
        .unwrap();
    let root = collection.find_one(doc! {}).await.unwrap().unwrap();
    let root = crate::store::sandboxes::read_sandbox(&root).unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: root.id,
        sandbox_created_at: root.created_at,
        owner: ResourceOwner::Organization {
            subject: root.owner,
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "cma:authenticated-provider-binding-manager".into(),
            organization: SubjectId::new("organization"),
            admitted_by: ResourceActor::Api {
                subject: SubjectId::new("a"),
                delegation_id: None,
            },
            verified_at: NOW.into(),
        }),
        revision: 1,
    };
    put(
        &db,
        "resource_policies",
        crate::store::resource_access::records::policy_document(&policy).unwrap(),
    )
    .await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    assert!(contexts[0].sandboxes.is_empty());
    assert!(contexts[0]
        .organization_sandboxes
        .contains("organization-root"));
    progress::capacity(&contexts).unwrap();
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_revision_pages_resume_with_a_scalar_position_instead_of_growing_exclusions() {
    let seed = seeded("round3_scalar_after").await;
    let db = seed.client.database(&seed.database);
    let collection = db.collection::<Document>("agent_profile_revisions");
    let original = collection.find_one(doc! {}).await.unwrap().unwrap();
    collection
        .insert_many((2..=1101).map(|n| {
            let mut row = original.clone();
            row.insert("_id", format!("{}:{n:010}", seed.target.agent_profile.id));
            row.insert("revision", n);
            row.get_document_mut("record")
                .unwrap()
                .insert("revision", n);
            row
        }))
        .await
        .unwrap();
    db.collection::<Document>("agents").insert_many((1..=1101).map(|n| doc! {"_id":format!("foreign-{n:05}"),"owner":"visitor","record":{"profile":{"id":seed.target.agent_profile.id.as_str(),"revision":n}}})).await.unwrap();
    let mut cursor = Cursor::default();
    let mut inventory = ErasureInventory::default();
    let mut complete = false;
    for _ in 0..1000 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
        assert!(
            !matches!(cursor.after, Some(Bson::Array(_))),
            "unchanged revisions must not accumulate an exclusion array"
        );
        assert!(serde_json::to_vec(&cursor.after).unwrap().len() < 4096);
        if !batch.has_more {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 1101);
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1101
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn protected_indexed_audit_pages_never_accumulate_exclusion_arrays() {
    use layer2_core::{
        resource_access::{
            OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
        },
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let db = database().await;
    sandbox(&db, "organization", "organization-root").await;
    let root = db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":"organization-root"})
        .await
        .unwrap()
        .unwrap();
    let root = crate::store::sandboxes::read_sandbox(&root).unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: root.id.clone(),
        sandbox_created_at: root.created_at,
        owner: ResourceOwner::Organization {
            subject: root.owner,
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "cma:authenticated-provider-binding-manager".into(),
            organization: SubjectId::new("organization"),
            admitted_by: ResourceActor::Api {
                subject: SubjectId::new("a"),
                delegation_id: None,
            },
            verified_at: NOW.into(),
        }),
        revision: 1,
    };
    put(
        &db,
        "resource_policies",
        crate::store::resource_access::records::policy_document(&policy).unwrap(),
    )
    .await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("a"))
        .on(ActivityTarget::Resource(
            "sandbox:organization-root:events".into(),
        ));
    store.append(&record).await.unwrap();
    let activity = db.collection::<Document>("activity");
    let base = activity.find_one(doc! {}).await.unwrap().unwrap();
    activity
        .insert_many((0..1100).map(|n| {
            let mut row = base.clone();
            let id = format!("act_protected_{n:05}");
            row.insert("_id", &id);
            row.get_document_mut("record").unwrap().insert("id", id);
            row
        }))
        .await
        .unwrap();
    let subject = SubjectId::new("a");
    let mut cursor = Cursor::default();
    let mut inventory = ErasureInventory::default();
    let mut complete = false;
    for _ in 0..1000 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        progress::acknowledge(&mut inventory, &batch, subject.as_str());
        assert!(
            !matches!(cursor.after, Some(Bson::Array(_))),
            "protected indexed audits must use bounded scalar continuation"
        );
        assert!(serde_json::to_vec(&cursor.after).unwrap().len() < 4096);
        if !batch.has_more {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(activity.count_documents(doc! {}).await.unwrap(), 1101);
    assert_eq!(
        activity
            .count_documents(doc! {"record.actor":{"$ne":layer2_core::DELETED_ACCOUNT_SUBJECT}})
            .await
            .unwrap(),
        0
    );
    assert!(db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":"organization-root","owner":"organization"})
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}

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
