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
    let token = claimed.work.lease.as_ref().unwrap().token.clone();
    assert_eq!(
        store
            .commit(claimed, vec![], original.revision, Some(&token))
            .await,
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
    let token = claimed.work.lease.as_ref().unwrap().token.clone();
    let event = claimed.complete(fence, &at, retention).unwrap();
    store
        .commit(claimed, vec![event], revision, Some(&token))
        .await
        .unwrap();
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
    let owner = SubjectId::new("owner");
    for marked in [true, false] {
        let update = if marked {
            doc! {"$set":{"active_subject_hash":subject_hash(&owner)}}
        } else {
            doc! {"$unset":{"active_subject_hash":""}}
        };
        db.collection::<Document>(RECORDS)
            .update_one(doc! {"_id":original.id.as_str()}, update)
            .await
            .unwrap();
        assert_eq!(
            store
                .active_hashes()
                .await
                .unwrap()
                .contains(&subject_hash(&owner)),
            marked
        );
        assert_eq!(store.erasing(&owner).await.unwrap(), marked);
        assert!(!store.erasing(&SubjectId::new("other")).await.unwrap());
        let mut session = db.client().start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        assert_eq!(
            erasing_in_session(&db, &owner, &mut session).await.unwrap(),
            marked
        );
        session.abort_transaction().await.unwrap();
    }
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
    assert!(store.replay(&owner, "first").await.unwrap().is_some());
    assert!(store.replay(&owner, "different").await.unwrap().is_none());
    assert!(store
        .replay(&SubjectId::new("other"), "first")
        .await
        .unwrap()
        .is_none());
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
async fn malformed_active_hash_keeps_its_subject_denied_without_blocking_other_rows() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    store.request(&owner, "first", NOW).await.unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"subject_hash":subject_hash(&owner)},
            doc! {"$set":{"active_subject_hash":"e".repeat(64)}},
        )
        .await
        .unwrap();
    let other = SubjectId::new("other");
    store.request(&other, "other", NOW).await.unwrap();
    let snapshot = store.active_hashes().await.unwrap();
    assert_eq!(
        snapshot,
        BTreeSet::from([subject_hash(&owner), subject_hash(&other)])
    );
    let live_snapshot = std::sync::Arc::new(crate::auth::account_standing::ErasingSnapshot::new(
        std::sync::Arc::new(store.clone()),
    ));
    let refresh = live_snapshot.start().await;
    assert!(live_snapshot.erasing(&owner).await.unwrap());
    assert!(live_snapshot.erasing(&other).await.unwrap());
    assert!(!live_snapshot
        .erasing(&SubjectId::new("absent"))
        .await
        .unwrap());
    refresh.abort();
    assert!(store.active(&owner).await.is_err());
    assert!(store.erasing(&owner).await.unwrap());
    assert!(!store.erasing(&SubjectId::new("absent")).await.unwrap());
    assert!(store.request(&owner, "different", NOW).await.is_err());
    assert_eq!(
        db.collection::<Document>(RECORDS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(erasing_in_session(&db, &owner, &mut session).await.unwrap());
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_snapshot_row_without_a_valid_subject_does_not_poison_refresh() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    store.request(&owner, "original", NOW).await.unwrap();
    db.collection::<Document>(RECORDS).insert_one(doc! {
        "_id":"broken-row", "subject_hash":"invalid", "active_subject_hash":"invalid", "status":"running"
    }).await.unwrap();
    assert_eq!(
        store.active_hashes().await.unwrap(),
        BTreeSet::from([subject_hash(&owner)])
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claim_due_skips_a_malformed_oldest_row_and_claims_the_next_valid_record() {
    let (_, db, store) = fixture().await;
    let bad = store
        .request(&SubjectId::new("broken-owner"), "broken", NOW)
        .await
        .unwrap();
    let good = store
        .request(&SubjectId::new("healthy-owner"), "healthy", LATER)
        .await
        .unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":bad.id.as_str()},
            doc! {"$set":{"document_schema_version":999_i64}},
        )
        .await
        .unwrap();
    let claimed = store
        .claim_due("worker", LATER, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, good.id);
    assert!(store.get(&bad.id).await.is_err());
    assert!(store
        .claim_due("other", LATER, Duration::from_secs(90))
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn claim_due_skips_corrupt_replay_keys_without_releasing_the_subject_gate() {
    for mutation in [
        doc! {"$unset":{"request_keys":""}},
        doc! {"$set":{"request_keys":mongodb::bson::Bson::Null}},
        doc! {"$set":{"request_keys":[7_i64]}},
    ] {
        let (_, db, store) = fixture().await;
        let owner = SubjectId::new("broken-replay-owner");
        let bad = store.request(&owner, "broken", NOW).await.unwrap();
        let good = store
            .request(&SubjectId::new("healthy-owner"), "healthy", LATER)
            .await
            .unwrap();
        db.collection::<Document>(RECORDS)
            .update_one(doc! {"_id":bad.id.as_str()}, mutation)
            .await
            .unwrap();
        assert!(store.erasing(&owner).await.unwrap());
        assert!(store
            .active_hashes()
            .await
            .unwrap()
            .contains(&subject_hash(&owner)));
        assert_eq!(
            store
                .claim_due("worker", LATER, Duration::from_secs(90))
                .await
                .unwrap()
                .unwrap()
                .id,
            good.id
        );
        assert!(store
            .claim_due("other", LATER, Duration::from_secs(90))
            .await
            .unwrap()
            .is_none());
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unknown_fields_are_quarantined_while_the_subject_remains_gated() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("future-owner");
    let future = store.request(&owner, "future", NOW).await.unwrap();
    let healthy = store
        .request(&SubjectId::new("healthy-owner"), "healthy", LATER)
        .await
        .unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":future.id.as_str()},
            doc! {"$set":{"future_schema_field":true}},
        )
        .await
        .unwrap();
    assert!(store.active(&owner).await.is_err());
    assert!(store.erasing(&owner).await.unwrap());
    assert!(store
        .active_hashes()
        .await
        .unwrap()
        .contains(&subject_hash(&owner)));
    assert_eq!(
        store
            .claim_due("worker", LATER, Duration::from_secs(90))
            .await
            .unwrap()
            .unwrap()
            .id,
        healthy.id
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn active_new_keys_return_existing_without_growing_persisted_keys() {
    let (_, db, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let original = store.request(&owner, "original", NOW).await.unwrap();
    for index in 0..32 {
        let key = format!("new-{index}");
        assert_eq!(
            store.request(&owner, &key, NOW).await.unwrap().id,
            original.id
        );
        assert!(store.replay(&owner, &key).await.unwrap().is_none());
    }
    let row = db
        .collection::<Document>(RECORDS)
        .find_one(doc! {"_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_array("request_keys").unwrap().len(), 1);
    assert_eq!(
        store.replay(&owner, "original").await.unwrap().unwrap().id,
        original.id
    );
    assert!(store
        .replay(&SubjectId::new("other"), "original")
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn renew_and_defer_commit_without_audit_sequence_gaps() {
    let (_, db, store) = fixture().await;
    let original = store
        .request(&SubjectId::new("owner"), "original", NOW)
        .await
        .unwrap();
    let mut record = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    record
        .renew(lease.fence, "2026-11-14T00:01:00Z", Duration::from_secs(90))
        .unwrap();
    store
        .commit(record.clone(), vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    let revision = record.revision;
    record.defer(lease.fence, "2026-11-14T00:03:00Z").unwrap();
    store
        .commit(record, vec![], revision, Some(&lease.token))
        .await
        .unwrap();
    assert!(store
        .claim_due("other", LATER, Duration::from_secs(90))
        .await
        .unwrap()
        .is_none());
    let claimed = store
        .claim_due("other", "2026-11-14T00:03:00Z", Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.event_seq, 3);
    assert_eq!(
        store
            .events(&original.id)
            .await
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn commit_compares_the_callers_lease_token_with_persisted_authority() {
    let (_, db, store) = fixture().await;
    let original = store
        .request(&SubjectId::new("owner"), "original", NOW)
        .await
        .unwrap();
    let mut record = store
        .claim_due("worker", NOW, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    record
        .renew(lease.fence, NOW, Duration::from_secs(90))
        .unwrap();
    assert_eq!(
        store
            .commit(record.clone(), vec![], revision, Some("wrong-token"))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":original.id.as_str()},
            doc! {"$set":{"work.lease.token":"replacement-token"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .commit(record, vec![], revision, Some(&lease.token))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(
        store.get(&original.id).await.unwrap().unwrap().revision,
        revision
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn standing_predicates_deny_noncompleted_records_without_active_markers() {
    let (_, db, store) = fixture().await;
    let mut expected = BTreeSet::new();
    for (name, mutation) in [
        ("running", doc! {"$unset":{"active_subject_hash":""}}),
        (
            "blocked",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":"blocked"}},
        ),
        (
            "unknown-status",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":"future-state"}},
        ),
        (
            "missing-status",
            doc! {"$unset":{"active_subject_hash":"","status":""}},
        ),
        (
            "mixed-array-status",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":["completed","running"]}},
        ),
        (
            "completed-array-status",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":["completed"]}},
        ),
        (
            "empty-array-status",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":mongodb::bson::Bson::Array(Vec::new())}},
        ),
        (
            "null-status",
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":null}},
        ),
    ] {
        let subject = SubjectId::new(name);
        let record = store.request(&subject, name, NOW).await.unwrap();
        db.collection::<Document>(RECORDS)
            .update_one(doc! {"_id":record.id.as_str()}, mutation)
            .await
            .unwrap();
        expected.insert(subject_hash(&subject));
        assert!(store.erasing(&subject).await.unwrap(), "{name}");
        let mut session = db.client().start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        assert!(fence_subject_in_session(&db, &subject, &mut session)
            .await
            .unwrap());
        session.abort_transaction().await.unwrap();
    }
    let completed_subject = SubjectId::new("completed");
    let mut completed = AccountDeletion::request(
        AccountDeletionId::generate(),
        completed_subject.clone(),
        NOW,
    )
    .unwrap();
    completed.step = ErasureStep::VerifyNoResidue;
    completed.residue = Some(ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some(NOW.into()),
        },
        ..ResidueReport::default()
    });
    let fence = completed
        .claim("worker", NOW, Duration::from_secs(90))
        .unwrap();
    completed
        .complete(fence, NOW, Duration::from_secs(37 * 86400))
        .unwrap();
    db.collection::<Document>(RECORDS)
        .insert_one(record_document(&completed, &[key_hash("completed")]).unwrap())
        .await
        .unwrap();
    let snapshot = store.active_hashes().await.unwrap();
    assert_eq!(snapshot, expected);
    assert!(!store.erasing(&completed_subject).await.unwrap());
    let live_snapshot = std::sync::Arc::new(crate::auth::account_standing::ErasingSnapshot::new(
        std::sync::Arc::new(store.clone()),
    ));
    let refresh = live_snapshot.start().await;
    for name in [
        "running",
        "blocked",
        "unknown-status",
        "missing-status",
        "mixed-array-status",
        "completed-array-status",
        "empty-array-status",
        "null-status",
    ] {
        assert!(live_snapshot.erasing(&SubjectId::new(name)).await.unwrap());
    }
    assert!(!live_snapshot.erasing(&completed_subject).await.unwrap());
    assert!(!live_snapshot
        .erasing(&SubjectId::new("absent"))
        .await
        .unwrap());
    refresh.abort();
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(
        !fence_subject_in_session(&db, &completed_subject, &mut session)
            .await
            .unwrap()
    );
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

fn plan_has_stage(value: &mongodb::bson::Bson, stage: &str) -> bool {
    match value {
        mongodb::bson::Bson::Document(document) => {
            document.get_str("stage").ok() == Some(stage)
                || document.values().any(|value| plan_has_stage(value, stage))
        }
        mongodb::bson::Bson::Array(values) => {
            values.iter().any(|value| plan_has_stage(value, stage))
        }
        _ => false,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn standing_queries_use_indexes_and_project_only_authority() {
    use mongodb::{
        event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client,
    };
    use std::sync::{Arc, Mutex};
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let db = client.database(&format!(
        "cma_acd_projection_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = MongoAccountDeletionStore::open(&db).await.unwrap();
    let owner = SubjectId::new("owner");
    store.request(&owner, "original", NOW).await.unwrap();
    let array_owner = SubjectId::new("mixed-array-owner");
    let array_record = store.request(&array_owner, "array", NOW).await.unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":array_record.id.as_str()},
            doc! {"$unset":{"active_subject_hash":""},"$set":{"status":["completed","running"]}},
        )
        .await
        .unwrap();
    let mut history = Vec::new();
    for index in 0..1024 {
        history.push(doc! {
            "_id":format!("historical-{index}"),
            "subject_hash":subject_hash(&SubjectId::new(format!("historical-{index}"))),
            "status":"completed",
            "request_keys":[format!("historical-key-{index}")],
        });
    }
    db.collection::<Document>(RECORDS)
        .insert_many(history)
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    assert_eq!(
        store.active_hashes().await.unwrap(),
        BTreeSet::from([subject_hash(&owner), subject_hash(&array_owner)])
    );
    assert!(store.erasing(&owner).await.unwrap());
    let mut session = db.client().start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert!(erasing_in_session(&db, &owner, &mut session).await.unwrap());
    session.abort_transaction().await.unwrap();
    let reads = commands.lock().unwrap().clone();
    assert!(reads.len() >= 3);
    for read in reads {
        let projection = read.get_document("projection").unwrap();
        assert!(!projection.contains_key("inventory"));
        assert!(!projection.contains_key("request_keys"));
        let mut find = Document::new();
        for key in [
            "find",
            "filter",
            "projection",
            "hint",
            "limit",
            "singleBatch",
        ] {
            if let Some(value) = read.get(key) {
                find.insert(key, value.clone());
            }
        }
        let explain = db
            .run_command(doc! {"explain":find,"verbosity":"executionStats"})
            .await
            .unwrap();
        let plan = explain
            .get_document("queryPlanner")
            .unwrap()
            .get("winningPlan")
            .unwrap();
        assert!(plan_has_stage(plan, "IXSCAN"), "{explain:?}");
        assert!(!plan_has_stage(plan, "COLLSCAN"), "{explain:?}");
        let statistics = explain.get_document("executionStats").unwrap();
        for counter in ["totalDocsExamined", "totalKeysExamined"] {
            let examined = statistics
                .get_i64(counter)
                .or_else(|_| statistics.get_i32(counter).map(i64::from))
                .unwrap();
            assert!(
                examined < 10,
                "standing must not scan tombstones: {explain:?}"
            );
        }
    }
    let indexes = db
        .collection::<Document>(RECORDS)
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(indexes.iter().any(|index| {
        index
            .options
            .as_ref()
            .and_then(|options| options.name.as_deref())
            == Some("account_deletions_status_subject")
            && index.keys == doc! {"status":1,"subject_hash":1}
    }));
    assert!(indexes.iter().any(|index| {
        index.options.as_ref().is_some_and(|options| {
            options.name.as_deref() == Some("account_deletions_array_status_subject")
                && options.partial_filter_expression == Some(doc! {"status":{"$type":"array"}})
                && !options.unique.unwrap_or(false)
                && options.expire_after.is_none()
        }) && index.keys == doc! {"subject_hash":1}
    }));
    db.drop().await.unwrap();
}
