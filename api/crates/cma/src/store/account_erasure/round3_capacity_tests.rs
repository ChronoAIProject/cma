//! Capacity rejection follows the actual section and never grows skip lists.
use super::{tests::*, *};
use mongodb::bson::{doc, Document};

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
