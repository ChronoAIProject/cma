use super::*;

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
        .claim_due("worker", &database_now(&db).await, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, good.id);
    assert!(store.get(&bad.id).await.is_err());
    assert!(store
        .claim_due("other", &database_now(&db).await, Duration::from_secs(90))
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
                .claim_due("worker", &database_now(&db).await, Duration::from_secs(90))
                .await
                .unwrap()
                .unwrap()
                .id,
            good.id
        );
        assert!(store
            .claim_due("other", &database_now(&db).await, Duration::from_secs(90))
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
            .claim_due("worker", &database_now(&db).await, Duration::from_secs(90))
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
