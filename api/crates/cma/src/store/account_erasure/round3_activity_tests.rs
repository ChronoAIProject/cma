//! All operator audit targets retain their evidence while explicit subjects are erased.
use super::{references::Redaction, tests::*, *};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget, DetailKey,
    DELETED_ACCOUNT_SUBJECT,
};
use mongodb::bson::{self, doc, Document};

// Targets emitted by SandboxOperatorService and the admin runtime update route.
const AUDITS: &[(ActivityAction, &str)] = &[
    (ActivityAction::List, "profiles"),
    (ActivityAction::Delete, "profile:agent:agp_removed"),
    (ActivityAction::Delete, "profile:workspace:wsp_removed"),
    (ActivityAction::Delete, "profile:sandbox:sbp_removed"),
    (ActivityAction::List, "sandboxes"),
    (ActivityAction::Read, "sandbox:removed-sandbox"),
    (ActivityAction::Update, "sandbox:updated-sandbox"),
    (ActivityAction::Read, "sandbox:removed-sandbox:events"),
];

fn activity(action: ActivityAction, target: &str, subject: Option<&str>) -> ActivityRecord {
    let mut record = ActivityRecord::new(action, ActivityOutcome::Succeeded)
        .by(SubjectId::new("operator"))
        .on(ActivityTarget::Resource(target.into()));
    record.at = NOW.into();
    if let Some(subject) = subject {
        record.detail.record(DetailKey::Subject, subject);
    }
    record.detail.record(DetailKey::Limit, "100");
    record.detail.record(DetailKey::Status, "completed");
    record
        .detail
        .record(DetailKey::CorrelationId, "operator-audit");
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

fn redacted(mut original: Document, record: &ActivityRecord) -> Document {
    let mut expected = record.clone();
    expected
        .detail
        .record(DetailKey::Subject, DELETED_ACCOUNT_SUBJECT);
    original.insert("record", bson::serialize_to_bson(&expected).unwrap());
    original.insert("mutation_version", 1_i64);
    original
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn all_admin_target_shapes_redact_explicit_subject_and_preserve_audit_evidence() {
    let db = database().await;
    let mut originals = vec![];
    for &(action, target) in AUDITS {
        let record = activity(action, target, Some("a"));
        let stored = append(&db, &record).await;
        originals.push((record, stored));
    }
    let mut inventory = ErasureInventory::default();
    drain(&db, "a", &mut inventory).await;
    assert_eq!(count(&db, "activity").await, AUDITS.len() as u64);
    for (record, stored) in originals {
        assert_eq!(
            row(&db, &record.id.0).await,
            redacted(stored, &record),
            "audit evidence must survive for {:?}",
            record.target
        );
    }
    assert_eq!(inventory, ErasureInventory::default());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn all_admin_target_shapes_preserve_unrelated_or_absent_explicit_subjects() {
    let db = database().await;
    let mut originals = vec![];
    for &(action, target) in AUDITS {
        for subject in [Some("b"), None] {
            let mut record = activity(action, target, subject);
            record.detail.record(DetailKey::ResourceName, "a");
            let stored = append(&db, &record).await;
            originals.push((record.id.0, stored));
        }
    }
    drain(&db, "a", &mut ErasureInventory::default()).await;
    for (id, stored) in originals {
        assert_eq!(row(&db, &id).await, stored);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn all_admin_target_shapes_use_exact_cas_before_redacting_subject() {
    let db = database().await;
    let subjects = BTreeSet::from(["a".into()]);
    for &(action, target) in AUDITS {
        let original = activity(action, target, Some("a"));
        let stale = append(&db, &original).await;
        let mut concurrent = original.clone();
        concurrent.outcome = ActivityOutcome::Failed("operator-followup".into());
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
        let current = row(&db, &original.id.0).await;
        assert_eq!(
            super::identity_refs::redact(&db, "activity", &stale, &subjects)
                .await
                .unwrap(),
            Redaction::Conflict,
            "stale audit must not overwrite concurrent changes for {target}"
        );
        assert_eq!(row(&db, &original.id.0).await, current);
        assert_eq!(
            super::identity_refs::redact(&db, "activity", &current, &subjects)
                .await
                .unwrap(),
            Redaction::Updated
        );
        assert_eq!(
            row(&db, &original.id.0).await,
            redacted(current, &concurrent)
        );
    }
    db.drop().await.unwrap();
}
