//! Real account populations exercise attributed capacity and bounded cursor recovery.
mod inventory_tests;

use super::{tests::*, *};
use layer2_core::{
    account_deletion::subject_hash,
    resource_access::{
        OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
    },
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget, SandboxId,
};
use mongodb::bson::{doc, Bson, Document};

fn assert_capacity(error: Error, subject: &str, class: &str, limit: u64) {
    let Error::Blocked(block) = error else {
        panic!("capacity must be an attributed permanent block, got {error:?}");
    };
    assert_eq!(block.subject_hash, subject_hash(&SubjectId::new(subject)));
    assert_eq!(
        block.reason,
        ErasureBlock::CapacityExceeded {
            class: class.into(),
            limit,
        }
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn oversized_owned_profile_population_is_blocked_only_for_its_owner() {
    let db = database().await;
    db.collection::<Document>("agent_profiles")
        .insert_many((0..20_001).map(|n| {
            doc! {
                "_id":format!("agp_{n}"),"owner":"a","record":{},
            }
        }))
        .await
        .unwrap();
    put(
        &db,
        "agent_profiles",
        doc! {"_id":"agp_other_owner","owner":"b","record":{}},
    )
    .await;
    let subjects = BTreeSet::from(["a".into(), "b".into()]);
    let error = profile_refs::retained(&db, &subjects).await.unwrap_err();
    assert_capacity(error, "a", "owned Profiles", 20_000);
    assert_eq!(count(&db, "agent_profiles").await, 20_002);

    db.collection::<Document>("agent_profiles")
        .delete_one(doc! {"_id":"agp_20000"})
        .await
        .unwrap();
    assert!(profile_refs::retained(&db, &subjects)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(count(&db, "agent_profiles").await, 20_001);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn large_organization_descendants_never_enter_the_personal_cache() {
    let db = database().await;
    sandbox(&db, "organization", "sbx_organization").await;
    let root = db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":"sbx_organization"})
        .await
        .unwrap()
        .unwrap();
    let root = crate::store::sandboxes::read_sandbox(&root).unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: SandboxId("sbx_organization".into()),
        sandbox_created_at: root.created_at,
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("organization"),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "cma:authenticated-provider-binding-manager".into(),
            organization: SubjectId::new("organization"),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("a"),
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
    store
        .append(
            &ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
                .by(SubjectId::new("a"))
                .on(ActivityTarget::Resource(
                    "sandbox:sbx_organization:events".into(),
                )),
        )
        .await
        .unwrap();
    db.collection::<Document>("agents")
        .insert_many((0..20_001).map(|n| doc! {
            "_id":format!("agt_organization_{n}"),"owner":"organization","sandbox_id":"sbx_organization",
        }))
        .await
        .unwrap();
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
    let mut context = contexts.into_iter().next().unwrap();
    for _ in 0..2 {
        context.refresh(&db).await.unwrap();
    }
    assert_eq!(context.sets.values().map(BTreeSet::len).sum::<usize>(), 0);
    assert!(context.protected.contains("sbx_organization"));
    assert!(context.organization_sandboxes.contains("sbx_organization"));
    assert!(context.protected.len() <= 2);
    assert!(context.sandboxes.is_empty());
    progress::capacity(std::slice::from_ref(&context)).unwrap();
    let mut cursor = Cursor {
        subjects: BTreeSet::from(["a".into()]),
        contexts: vec![context],
        ..Default::default()
    };
    progress::cursor_capacity(&mut cursor).unwrap();
    assert!(serde_json::to_vec(&cursor).unwrap().len() < progress::MAX_CURSOR_BYTES);
    assert_eq!(count(&db, "agents").await, 20_001);
    db.drop().await.unwrap();
}

fn personal_context(subject: &str, count: usize) -> context::Context {
    let mut context = context::Context::default();
    context.subject = subject.into();
    context.sets.insert(
        catalog::InventorySet::Agents,
        (0..count)
            .map(|n| format!("{subject}-{n:05}-{}", "x".repeat(25)))
            .collect(),
    );
    context
}

#[test]
fn oversized_serialized_cohort_is_attributed_and_releases_cached_state() {
    let contexts = vec![personal_context("a", 15_001), personal_context("b", 15_000)];
    progress::capacity(&contexts).unwrap();
    for context in &contexts {
        assert!(serde_json::to_vec(context).unwrap().len() < progress::MAX_CURSOR_BYTES);
        let mut single = Cursor {
            subjects: BTreeSet::from([context.subject.clone()]),
            contexts: vec![context.clone()],
            ..Default::default()
        };
        let before = serde_json::to_vec(&single).unwrap();
        progress::cursor_capacity(&mut single).unwrap();
        assert_eq!(serde_json::to_vec(&single).unwrap(), before);
    }
    let mut cursor = Cursor {
        subjects: BTreeSet::from(["a".into(), "b".into()]),
        contexts,
        retained_profiles: Some(BTreeSet::from(["agp_cached:0000000001".into()])),
        operation: 37,
        after: Some(Bson::String("resumable-page".into())),
        ..Default::default()
    };
    assert!(serde_json::to_vec(&cursor).unwrap().len() > progress::MAX_CURSOR_BYTES);
    let error = progress::cursor_capacity(&mut cursor).unwrap_err();
    assert_capacity(
        error,
        "a",
        "serialized cohort cursor bytes",
        progress::MAX_CURSOR_BYTES as u64,
    );
    assert!(cursor.contexts.is_empty());
    assert!(cursor.retained_profiles.is_none());
    assert_eq!(cursor.operation, 0);
    assert!(cursor.after.is_none());
    assert!(serde_json::to_vec(&cursor).unwrap().len() <= progress::MAX_CURSOR_BYTES);
    progress::cursor_capacity(&mut cursor).unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_rejection_keeps_the_real_erasure_cursor_bounded_before_effects() {
    let db = database().await;
    let contexts = vec![personal_context("a", 15_001), personal_context("b", 15_000)];
    let canary = contexts[0]
        .sets
        .get(&catalog::InventorySet::Agents)
        .unwrap()
        .first()
        .unwrap();
    put(&db, "agents", doc! {"_id":canary,"owner":"a"}).await;
    let mut cursor = Cursor::default();
    progress::prepare(
        &mut cursor,
        BTreeSet::from(["a".into(), "b".into()]),
        Options {
            allocation_rollups_enabled: Some(false),
            agent_shared_observation: Some(false),
            retain_legal_consents: false,
            drain_qualification: Some(TransactionDrainQualification::fixture()),
            billing_enabled: false,
            transaction_drain_minimum_secs: 120,
        },
        &progress::operations(),
    );
    cursor.contexts = contexts;
    cursor.retained_profiles = Some(BTreeSet::from(["agp_cached:0000000001".into()]));
    cursor.drain_complete = true;
    cursor.after = Some(Bson::String("cached-page".into()));
    let a = SubjectId::new("a");
    let b = SubjectId::new("b");
    let inventory = ErasureInventory::default();
    let error = erase_records(
        &db,
        &[
            Target {
                subject: &a,
                inventory: &inventory,
            },
            Target {
                subject: &b,
                inventory: &inventory,
            },
        ],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap_err();
    assert_capacity(
        error,
        "a",
        "serialized cohort cursor bytes",
        progress::MAX_CURSOR_BYTES as u64,
    );
    assert_eq!(count(&db, "agents").await, 1);
    assert!(cursor.contexts.is_empty());
    assert!(cursor.retained_profiles.is_none());
    assert_eq!(cursor.operation, 0);
    assert!(cursor.after.is_none());
    assert!(serde_json::to_vec(&cursor).unwrap().len() <= progress::MAX_CURSOR_BYTES);
    assert!(mongodb::bson::serialize_to_vec(&cursor).unwrap().len() <= progress::MAX_CURSOR_BYTES);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn corrupt_activity_page_boundary_is_attributed_to_its_actor() {
    let db = database().await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    store
        .append(
            &ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
                .by(SubjectId::new("b"))
                .on(ActivityTarget::Resource("sandbox:unknown".into())),
        )
        .await
        .unwrap();
    let activity = db.collection::<Document>("activity");
    let base = activity.find_one(doc! {}).await.unwrap().unwrap();
    activity
        .insert_many((1..500).map(|n| {
            let mut row = base.clone();
            let id = format!("act_b_{n:03}");
            row.insert("_id", &id);
            row.get_document_mut("record").unwrap().insert("id", id);
            if n == 499 {
                row.insert("recorded_at", "damaged-timestamp");
            }
            row
        }))
        .await
        .unwrap();
    let a = SubjectId::new("a");
    let b = SubjectId::new("b");
    let inventory = ErasureInventory::default();
    let error = context::load_cohort(
        &db,
        &[
            Target {
                subject: &a,
                inventory: &inventory,
            },
            Target {
                subject: &b,
                inventory: &inventory,
            },
        ],
    )
    .await
    .unwrap_err();
    let Error::Blocked(block) = error else {
        panic!("corrupt activity must be an attributed block, got {error:?}");
    };
    assert_eq!(block.subject_hash, subject_hash(&b));
    assert_eq!(
        block.reason,
        ErasureBlock::CorruptRecord {
            collection: "activity".into(),
        }
    );
    assert_eq!(count(&db, "activity").await, 500);
    db.drop().await.unwrap();
}
