//! Operator activity keeps its audit identity while explicit subject details are erased.
use super::{references::Redaction, tests::*, *};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget, DetailKey,
    DELETED_ACCOUNT_SUBJECT,
};
use mongodb::bson::{self, doc, Document};

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
async fn purged_sandbox_operator_audit_redacts_only_the_explicit_subject() {
    let db = database().await;
    let mut originals = vec![];
    for target in ["sandbox:removed-sandbox", "sandbox:removed-sandbox:events"] {
        let record = activity("operator", target, "a");
        append(&db, &record).await;
        originals.push(record);
    }
    let unrelated = activity("operator", "sandbox:removed-sandbox", "b");
    let untouched = append(&db, &unrelated).await;
    let mut inventory = ErasureInventory::default();
    drain(&db, "a", &mut inventory).await;
    for mut expected in originals {
        expected
            .detail
            .record(DetailKey::Subject, DELETED_ACCOUNT_SUBJECT);
        let stored = row(&db, &expected.id.0).await;
        let actual: ActivityRecord = crate::store::sandbox_mongo::read_record(&stored).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(stored.get_i64("mutation_version").unwrap(), 1);
    }
    assert_eq!(row(&db, &unrelated.id.0).await, untouched);
    assert!(inventory.sandboxes.is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_detail_redaction_cannot_overwrite_a_concurrent_audit_edit() {
    let db = database().await;
    let original = activity("operator", "sandbox:removed-sandbox", "a");
    let stale = append(&db, &original).await;
    let mut concurrent = original.clone();
    concurrent
        .detail
        .record(DetailKey::Status, "concurrent-status");
    db.collection::<Document>("activity")
        .update_one(
            doc! {"_id":&original.id.0},
            doc! {"$set":{"record":bson::serialize_to_bson(&concurrent).unwrap()}},
        )
        .await
        .unwrap();
    let before = row(&db, &original.id.0).await;
    let subjects = BTreeSet::from(["a".into()]);
    assert_eq!(
        super::identity_refs::redact(&db, "activity", &stale, &subjects)
            .await
            .unwrap(),
        Redaction::Conflict
    );
    assert_eq!(row(&db, &original.id.0).await, before);
    assert_eq!(
        super::identity_refs::redact(&db, "activity", &before, &subjects)
            .await
            .unwrap(),
        Redaction::Updated
    );
    concurrent
        .detail
        .record(DetailKey::Subject, DELETED_ACCOUNT_SUBJECT);
    let actual: ActivityRecord =
        crate::store::sandbox_mongo::read_record(&row(&db, &original.id.0).await).unwrap();
    assert_eq!(actual, concurrent);
    db.drop().await.unwrap();
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
