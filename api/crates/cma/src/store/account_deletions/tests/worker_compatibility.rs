use super::worker_fixture::*;
use crate::store::account_deletions::commit_pause::{Pause, Point};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn enrolled_job_refuses_generic_replacement_and_claim_but_preserves_replay() {
    let (db, store, _, checkpoint) = enrolled().await;
    let original = store
        .get(&checkpoint.binding.deletion_id)
        .await
        .unwrap()
        .unwrap();
    let foreign = store
        .latest(&SubjectId::new("foreign-owner"))
        .await
        .unwrap()
        .unwrap();
    db.collection::<Document>(RECORDS)
        .update_one(
            doc! {"_id":foreign.id.as_str()},
            doc! {"$set":{"work.ready_at":"2099-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    let mut changed = original.clone();
    changed.revision += 1;
    let before = snapshot(&db).await;
    assert_eq!(
        store.commit(changed, vec![], original.revision, None).await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(
        store
            .claim_due("legacy", &database_now(&db).await, Duration::from_secs(90))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(snapshot(&db).await, before);
    assert_eq!(
        store
            .replay(&SubjectId::new("owner"), "accepted")
            .await
            .unwrap(),
        Some(original.clone())
    );
    assert!(store.erasing(&SubjectId::new("owner")).await.unwrap());
    // The retained strict parent decoder still round-trips the unchanged wire fields.
    let encoded = record_document(&original, &[key_hash("accepted")]).unwrap();
    assert_eq!(read_record(encoded.clone()).unwrap(), original);
    assert!(!encoded.contains_key("checkpoint") && !encoded.contains_key("journal_sequence"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn enrollment_serializes_with_a_generic_writer_that_read_no_sidecar() {
    let (_, db, mut generic) = fixture().await;
    crate::store::migrations::account_erasure_work::prepare_for_test(&db).await;
    let at = database_now(&db).await;
    let original = generic
        .request(&SubjectId::new("owner"), "accepted", &at)
        .await
        .unwrap();
    let mut claimed = generic
        .claim_due("legacy", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    let revision = claimed.revision;
    let lease = claimed.work.lease.clone().unwrap();
    claimed
        .renew(lease.fence, &at, Duration::from_secs(300))
        .unwrap();
    let pause = Pause::new(Point::BeforeGuard);
    generic.commit_pause = Some(pause.clone());
    let write = tokio::spawn(async move {
        generic
            .commit(claimed, vec![], revision, Some(&lease.token))
            .await
    });
    pause.reached().await;
    let journal = reopened(&db, None).await;
    let request = JournalRequest::new(binding(original.id), JournalCommand::Enroll).unwrap();
    journal.enroll(&request, &budget()).await.unwrap();
    let before = snapshot(&db).await;
    pause.release();
    assert_eq!(write.await.unwrap(), Err(AccountDeletionError::StaleFence));
    assert_eq!(snapshot(&db).await, before);
    db.drop().await.unwrap();
}
