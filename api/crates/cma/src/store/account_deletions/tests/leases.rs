use super::*;

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
