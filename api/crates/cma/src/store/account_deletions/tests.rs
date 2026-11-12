use super::*;
use layer2_core::account_deletion::{
    AccountDeletionStatus, AccountDeletionStore, ErasureStep, ResidueReport,
};
use std::time::Duration;
const NOW: &str = "2026-11-14T00:00:00Z";
const LATER: &str = "2026-11-14T00:02:00Z";
async fn fixture() -> (mongodb::Client, Database, MongoAccountDeletionStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!("cma_acd_{}", uuid::Uuid::new_v4().simple()));
    let store = MongoAccountDeletionStore::open(&db).await.unwrap();
    (client, db, store)
}
async fn owner_sandbox_ids(store: &crate::store::MongoSandboxStore) -> BTreeSet<String> {
    store
        .sandboxes
        .find(doc! {"owner":"owner"})
        .projection(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get_str("_id").unwrap().to_owned())
        .collect()
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn active_subject_hash_is_unique() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let (a, b) = tokio::join!(
        store.request(&owner, "first", NOW),
        store.request(&owner, "second", NOW)
    );
    assert_eq!(a.unwrap().id, b.unwrap().id);
    assert_eq!(
        db.collection::<Document>(RECORDS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claim_issues_new_lease_token_each_time() {
    let (_, db, store) = fixture().await;
    store
        .request(&SubjectId::new("owner"), "first", NOW)
        .await
        .unwrap();
    let first = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .claim_due("other", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .is_none());
    let second = store
        .claim_due("other", LATER, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        first.work.lease.unwrap().token,
        second.work.lease.unwrap().token
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn commit_with_stale_revision_conflicts() {
    let (_, db, store) = fixture().await;
    let original = store
        .request(&SubjectId::new("owner"), "first", NOW)
        .await
        .unwrap();
    let claimed = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.commit(claimed, vec![], original.revision).await,
        Err(AccountDeletionError::Conflict)
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn events_are_append_only_and_sequenced() {
    let (_, db, store) = fixture().await;
    let original = store
        .request(&SubjectId::new("owner"), "first", NOW)
        .await
        .unwrap();
    let claimed = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let events = store.events(&original.id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
    let mut duplicate = event_document(&events[0], None).unwrap();
    duplicate.insert("_id", "different-event-id");
    assert!(db
        .collection::<Document>(EVENTS)
        .insert_one(duplicate)
        .await
        .is_err());
    assert!(claimed.revision > original.revision);
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn completed_record_has_ttl_and_no_subject() {
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let retention = Duration::from_secs(37 * 86400);
    let original = store
        .request(&SubjectId::new("owner"), "first", &at)
        .await
        .unwrap();
    let mut claimed = store
        .claim_due("worker", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let revision = claimed.revision;
    claimed.step = ErasureStep::VerifyNoResidue;
    claimed.residue = Some(ResidueReport {
        keys: layer2_core::account_deletion::KeyReceipt {
            remaining: 0,
            empty_at: Some(at.clone()),
        },
        ..ResidueReport::default()
    });
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let event = claimed.complete(fence, &at, retention).unwrap();
    store.commit(claimed, vec![event], revision).await.unwrap();
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("subject"));
    assert!(!row.contains_key("active_subject_hash"));
    let expiry_seconds = layer2_core::seconds_from_rfc3339(&at).unwrap() + retention.as_secs();
    assert_eq!(
        row.get_datetime("expire_at").unwrap().timestamp_millis(),
        i64::try_from(expiry_seconds * 1000).unwrap()
    );
    assert_eq!(
        store.get(&original.id).await.unwrap().unwrap().status,
        AccountDeletionStatus::Completed
    );
    assert!(db
        .collection::<Document>(EVENTS)
        .find_one(doc! {"deletion_id":original.id.as_str(),"expire_at":{"$type":"date"}})
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deletion_and_admission_race_leaves_no_new_sandbox() {
    use crate::store::sandbox_admission_mongo_integration_tests::sandbox;
    use layer2_core::sandbox::store::{
        SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore, SandboxStoreError,
    };
    let (client, db, deletions) = fixture().await;
    let sandboxes = crate::store::MongoSandboxStore::open(&client, db.name())
        .await
        .unwrap();
    let admit = |index: usize| {
        let mut record = sandbox(
            &format!("sbx_deletion_{index}"),
            &format!("launch-deletion-{index}"),
        );
        record.name = "deletion".into();
        SandboxAdmission::new(
            record,
            SandboxAdmissionName::Allocate("deletion".into()),
            100,
            NOW.into(),
        )
        .unwrap()
    };
    let owner = SubjectId::new("owner");
    let (cutover, attempts) = tokio::join!(
        async {
            deletions.request(&owner, "delete", NOW).await.unwrap();
            owner_sandbox_ids(&sandboxes).await
        },
        futures_util::future::join_all((0..8).map(|index| sandboxes.admit_sandbox(admit(index))))
    );
    let mut admitted = 0;
    for attempt in attempts {
        match attempt {
            Ok(_) => admitted += 1,
            Err(SandboxStoreError::AccountDeletionInProgress) => {}
            Err(error) => panic!("unexpected admission error: {error}"),
        }
    }
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":"owner"})
            .await
            .unwrap(),
        admitted
    );
    assert_eq!(owner_sandbox_ids(&sandboxes).await, cutover);
    assert_eq!(
        sandboxes.admit_sandbox(admit(100)).await,
        Err(SandboxStoreError::AccountDeletionInProgress)
    );
    assert_eq!(
        sandboxes
            .sandboxes
            .count_documents(doc! {"owner":"owner"})
            .await
            .unwrap(),
        admitted
    );
    assert_eq!(owner_sandbox_ids(&sandboxes).await, cutover);
    let mut other = admit(101).sandbox;
    other.owner = SubjectId::new("other-owner");
    let other = SandboxAdmission::new(
        other,
        SandboxAdmissionName::Allocate("deletion".into()),
        100,
        NOW.into(),
    )
    .unwrap();
    assert!(sandboxes.admit_sandbox(other).await.is_ok());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persisted_request_keys_allow_only_exact_owner_replays() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    store.request(&owner, "first", NOW).await.unwrap();
    assert!(store.has_request_key(&owner, "first").await.unwrap());
    assert!(!store.has_request_key(&owner, "different").await.unwrap());
    assert!(!store
        .has_request_key(&SubjectId::new("other"), "first")
        .await
        .unwrap());
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{row:?}").contains("first"));
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn latest_prefers_active_over_same_second_completed_history() {
    use layer2_core::account_deletion::{AccountDeletion, AccountDeletionId, KeyReceipt};
    let (_, db, store) = fixture().await;
    let at = layer2_core::now_rfc3339();
    let owner = SubjectId::new("owner");
    let mut completed = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "f".repeat(32))),
        owner.clone(),
        &at,
    )
    .unwrap();
    completed.step = ErasureStep::VerifyNoResidue;
    completed.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(at.clone()),
        },
        ..ResidueReport::default()
    });
    let fence = completed
        .claim("worker", &at, Duration::from_secs(90))
        .unwrap();
    completed
        .complete(fence, &at, Duration::from_secs(37 * 86400))
        .unwrap();
    let active = AccountDeletion::request(
        AccountDeletionId(format!("acd_{}", "0".repeat(32))),
        owner.clone(),
        &at,
    )
    .unwrap();
    db.collection::<Document>(RECORDS)
        .insert_many([
            record_document(&completed, &[key_hash("completed")]).unwrap(),
            record_document(&active, &[key_hash("active")]).unwrap(),
        ])
        .await
        .unwrap();
    assert_eq!(store.latest(&owner).await.unwrap().unwrap().id, active.id);
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_active_hash_fails_closed_on_snapshot_and_point_reads() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    store.request(&owner, "first", NOW).await.unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {},
            doc! {"$set":{"active_subject_hash":"e".repeat(64)}},
        )
        .await
        .unwrap();
    assert!(store.active_hashes().await.is_err());
    assert!(store.active(&owner).await.is_err());
    assert!(store.request(&owner, "different", NOW).await.is_err());
    assert_eq!(
        db.collection::<Document>(RECORDS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(erasing_in_session(&db, &owner, &mut session).await.unwrap());
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}
