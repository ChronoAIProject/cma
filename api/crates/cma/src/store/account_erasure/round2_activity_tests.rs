//! Operator activity keeps its audit identity while explicit subject details are erased.
use super::{tests::*, *};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget, DetailKey,
};
use mongodb::bson::{doc, Document};

fn activity(actor: &str, target: &str, subject: &str) -> ActivityRecord {
    let mut record = ActivityRecord::new(ActivityAction::Delete, ActivityOutcome::Succeeded)
        .by(SubjectId::new(actor))
        .on(ActivityTarget::Resource(target.into()));
    record.at = NOW.into();
    record.detail.record(DetailKey::Subject, subject);
    record
        .detail
        .record(DetailKey::CorrelationId, "operator-audit");
    record.detail.record(DetailKey::Status, "completed");
    record
}

async fn append(db: &Database, record: &ActivityRecord) -> Document {
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    store.append(record).await.unwrap();
    row(db, &record.id.0).await
}

async fn row(db: &Database, id: &str) -> Document {
    db.collection::<Document>("activity")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_actor_audit_is_deleted_and_cannot_claim_its_target() {
    let db = database().await;
    let original = activity("a", "sandbox:removed-sandbox", "a");
    append(&db, &original).await;
    let mut inventory = ErasureInventory::default();
    drain(&db, "a", &mut inventory).await;
    assert_eq!(count(&db, "activity").await, 0);
    assert!(inventory.sandboxes.is_empty());
    db.drop().await.unwrap();
}

#[test]
fn malformed_or_template_target_never_establishes_sandbox_identity() {
    for target in [
        "sandbox:",
        "sandbox::events",
        "sandbox:{id}",
        "sandbox:id:unexpected",
        "sandbox:id:events:events",
        "sandbox: id",
        "sandbox:id/child",
        "/v1/sandboxes/{id}/runtime-update",
    ] {
        let row = doc! {"record":{"target":{"kind":"resource","id":target}}};
        assert_eq!(
            super::identity_activity::sandbox_target(&row),
            None,
            "{target}"
        );
    }
    for target in ["sandbox:concrete-id", "sandbox:concrete-id:events"] {
        let row = doc! {"record":{"target":{"kind":"resource","id":target}}};
        assert_eq!(
            super::identity_activity::sandbox_target(&row),
            Some("concrete-id")
        );
    }
}
