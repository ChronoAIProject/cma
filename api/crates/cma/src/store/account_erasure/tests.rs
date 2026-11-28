use super::*;
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

async fn once(
    db: &Database,
    subject: &SubjectId,
    inventory: &ErasureInventory,
    now: &str,
) -> Result<Batch, Error> {
    super::erase_records(
        db,
        &[Target { subject, inventory }],
        &mut Cursor::default(),
        now,
    )
    .await
}

pub(super) const NOW: &str = "2026-11-14T00:00:00Z";
pub(super) async fn database() -> Database {
    Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap()
        .database(&format!("cma_erasure_{}", uuid::Uuid::new_v4().simple()))
}
pub(super) async fn put(db: &Database, collection: &str, row: Document) {
    db.collection::<Document>(collection)
        .insert_one(row)
        .await
        .unwrap();
}
pub(super) async fn count(db: &Database, collection: &str) -> u64 {
    db.collection::<Document>(collection)
        .count_documents(doc! {})
        .await
        .unwrap()
}
pub(super) async fn sandbox(db: &Database, owner: &str, id: &str) {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    sandbox.owner = SubjectId::new(owner);
    sandbox.status = layer2_core::sandbox::SandboxStatus::Retired;
    sandbox.retired_at = Some(NOW.into());
    sandbox.purge_after = Some(NOW.into());
    sandbox.runtime = None;
    sandbox.credential.current = None;
    sandbox.credential.delivered = false;
    sandbox.work = layer2_core::sandbox::SandboxWorkState::new(
        layer2_core::sandbox::SandboxWorkStep::None,
        NOW.into(),
    );
    put(
        db,
        "sandboxes",
        crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap(),
    )
    .await;
}
pub(in crate::store) async fn drain(
    db: &Database,
    owner: &str,
    inventory: &mut ErasureInventory,
) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    let mut cursor = Cursor::default();
    for _ in 0..2000 {
        let subject = SubjectId::new(owner);
        let batch = super::erase_records(
            db,
            &[Target {
                subject: &subject,
                inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        super::progress::acknowledge(inventory, &batch, owner);
        for (collection, count) in batch.deleted {
            *counts.entry(collection).or_default() += count;
        }
        for id in batch.pending_key_ids.get(owner).into_iter().flatten() {
            if !inventory.pending_key_ids.contains(id) {
                inventory.pending_key_ids.push(id.clone());
            }
        }
        if !batch.has_more {
            return counts;
        }
    }
    panic!("erasure failed to finish within its bounded batch budget");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_created_after_inventory_is_still_erased() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_late", "sandbox_id":"sbx_a", "owner":"a"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":"late-log", "agent_id":"agt_late"},
    )
    .await;
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "agent_log").await, 0);
    assert_eq!(count(&db, "agents").await, 0);
    assert_eq!(count(&db, "sandboxes").await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn pending_billing_delivery_blocks() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    put(&db, "sandbox_billing_state", doc! {"_id":"sbx_a", "sandbox_id":"sbx_a", "owner_id":"a", "pending":{"request_id":"pending"},"windows":[]}).await;
    put(&db, "sandbox_usage", doc! {"_id":"sbx_a", "owner":"a"}).await;
    assert!(matches!(
        once(&db, &SubjectId::new("a"), &ErasureInventory::default(), NOW).await,
        Err(Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        }))
    ));
    assert_eq!(count(&db, "sandbox_usage").await, 1);
    assert_eq!(count(&db, "sandbox_billing_state").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn proven_absent_collection_with_owner_rows_blocks() {
    let db = database().await;
    put(
        &db,
        "bridge_definitions",
        doc! {"_id":"legacy", "owner":"a"},
    )
    .await;
    let foreign = doc! {"_id":"legacy_other", "owner":"b"};
    put(&db, "bridge_definitions", foreign.clone()).await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    let first = erase_records(&db, &targets, &mut cursor, NOW)
        .await
        .unwrap();
    assert!(first.has_more && first.deleted.is_empty() && first.redacted.is_empty());
    assert_eq!(count(&db, "bridge_definitions").await, 2);
    let mut rejection = None;
    for _ in 0..300 {
        match erase_records(&db, &targets, &mut cursor, NOW).await {
            Ok(batch) => {
                assert!(
                    batch.has_more,
                    "unsupported residue must never complete silently"
                );
                assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
                assert_eq!(count(&db, "bridge_definitions").await, 2);
            }
            Err(error) => {
                rejection = Some(error);
                break;
            }
        }
    }
    match rejection.expect("resumable discovery must reach the residue check") {
        Error::Blocked(AttributedBlock {
            subject_hash,
            reason: ErasureBlock::UnsupportedResidue { collection },
        }) => {
            assert_eq!(collection, "bridge_definitions");
            assert_eq!(
                subject_hash,
                layer2_core::account_deletion::subject_hash(&subject)
            );
        }
        error => panic!("unexpected residue result: {error:?}"),
    }
    assert_eq!(count(&db, "bridge_definitions").await, 2);
    db.collection::<Document>("bridge_definitions")
        .delete_one(doc! {"_id":"legacy","owner":"a"})
        .await
        .unwrap();
    let mut completed = false;
    for _ in 0..300 {
        let batch = erase_records(&db, &targets, &mut cursor, NOW)
            .await
            .unwrap();
        assert!(batch.deleted.is_empty() && batch.redacted.is_empty());
        if !batch.has_more {
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "removing unsupported personal residue permits continuation"
    );
    assert_eq!(count(&db, "bridge_definitions").await, 1);
    assert_eq!(
        db.collection::<Document>("bridge_definitions")
            .find_one(doc! {"_id":"legacy_other"})
            .await
            .unwrap(),
        Some(foreign)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revoke_debt_is_copied_before_sandbox_document_is_deleted() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"}, doc! {"$set":{"record.credential.revoke_debt":[{"provider_key_id":"debt-id", "queued_at":NOW}]}}).await.unwrap();
    let mut inventory = ErasureInventory::default();
    let mut discovered = false;
    for _ in 0..300 {
        let batch = once(&db, &SubjectId::new("a"), &inventory, NOW)
            .await
            .unwrap();
        if batch
            .pending_key_ids
            .get("a")
            .is_some_and(|keys| keys.contains(&"debt-id".to_string()))
        {
            assert_eq!(count(&db, "sandboxes").await, 1);
            discovered = true;
            inventory.pending_key_ids.extend(
                batch
                    .pending_key_ids
                    .get("a")
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            break;
        }
        assert!(batch.has_more);
    }
    assert!(discovered);
    drain(&db, "a", &mut inventory).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    db.drop().await.unwrap();
}

#[path = "execution_tests.rs"]
mod execution;
