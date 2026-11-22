use super::*;
use crate::store::account_deletions::commit_pause::{Pause, Point};

async fn second_store(db: &Database) -> MongoAccountDeletionStore {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    MongoAccountDeletionStore::from_database(&client.database(db.name()))
}

async fn claimed(store: &MongoAccountDeletionStore, db: &Database) -> AccountDeletion {
    let at = database_now(db).await;
    store
        .request(&SubjectId::new("owner"), "accepted", &at)
        .await
        .unwrap();
    store
        .request(&SubjectId::new("foreign-owner"), "foreign", &at)
        .await
        .unwrap();
    // Keep the sentinel outside claim selection without changing its standing.
    store
        .records
        .update_one(
            doc! {"subject": "foreign-owner"},
            doc! {"$set":{"work.ready_at":"2099-01-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    store
        .claim_due("first", &at, Duration::from_secs(4))
        .await
        .unwrap()
        .unwrap()
}

fn advance(mut record: AccountDeletion) -> (AccountDeletion, ErasureEvent) {
    let lease = record.work.lease.as_ref().unwrap();
    let at = record.requested_at.clone();
    let event = record
        .advance(lease.fence, &at, Default::default())
        .unwrap();
    (record, event)
}

async fn expired(db: &Database, record: &AccountDeletion) {
    let expiry = &record.work.lease.as_ref().unwrap().expires_at;
    tokio::time::timeout(Duration::from_secs(15), async {
        while database_now(db).await < *expiry {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("database time must pass the original lease deadline");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expiry_before_final_guard_aborts_record_and_staged_event() {
    let (_, db, mut store) = fixture().await;
    let original = claimed(&store, &db).await;
    let before = snapshot(&store).await;
    let second = second_store(&db).await;
    let pause = Pause::new(Point::BeforeGuard);
    store.commit_pause = Some(pause.clone());
    let (next, event) = advance(original.clone());
    let copy = original.clone();
    let write = tokio::spawn(async move {
        store
            .commit(
                next,
                vec![event],
                copy.revision,
                Some(&copy.work.lease.as_ref().unwrap().token),
            )
            .await
    });
    pause.reached().await;
    assert_eq!(snapshot(&second).await, before);
    expired(&db, &original).await;
    pause.release();
    assert_eq!(write.await.unwrap(), Err(AccountDeletionError::StaleFence));
    assert_eq!(snapshot(&second).await, before);
    let successor = second
        .claim_due(
            "successor",
            &database_now(&db).await,
            Duration::from_secs(90),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.id, original.id);
    assert_eq!(successor.step, original.step);
    assert!(successor.work.lease.unwrap().fence > original.work.lease.unwrap().fence);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admitted_transaction_serializes_takeover_after_lease_expiry() {
    let (_, db, mut store) = fixture().await;
    let original = claimed(&store, &db).await;
    let second = second_store(&db).await;
    let foreign = second
        .latest(&SubjectId::new("foreign-owner"))
        .await
        .unwrap();
    let pause = Pause::new(Point::AfterGuard);
    store.commit_pause = Some(pause.clone());
    let (next, event) = advance(original.clone());
    let committed = next.clone();
    let copy = original.clone();
    let write = tokio::spawn(async move {
        store
            .commit(
                next,
                vec![event],
                copy.revision,
                Some(&copy.work.lease.as_ref().unwrap().token),
            )
            .await
    });
    pause.reached().await;
    expired(&db, &original).await;
    assert_eq!(
        second
            .claim_due(
                "contender",
                &database_now(&db).await,
                Duration::from_secs(90)
            )
            .await,
        Err(AccountDeletionError::Conflict)
    );
    assert_eq!(
        second.get(&original.id).await.unwrap(),
        Some(original.clone())
    );
    pause.release();
    write.await.unwrap().unwrap();
    assert_eq!(
        second.get(&original.id).await.unwrap(),
        Some(committed.clone())
    );
    let successor = second
        .claim_due(
            "successor",
            &database_now(&db).await,
            Duration::from_secs(90),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.step, committed.step);
    assert_eq!(successor.event_seq, committed.event_seq + 1);
    assert_ne!(
        successor.work.lease.as_ref().unwrap().token,
        original.work.lease.as_ref().unwrap().token
    );
    let (stale, event) = advance(original.clone());
    assert_eq!(
        second
            .commit(
                stale,
                vec![event],
                original.revision,
                Some(&original.work.lease.as_ref().unwrap().token)
            )
            .await,
        Err(AccountDeletionError::Conflict)
    );
    assert_eq!(
        second
            .latest(&SubjectId::new("foreign-owner"))
            .await
            .unwrap(),
        foreign
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn committed_transition_is_visible_before_reply_and_stale_retry_cannot_duplicate_it() {
    let (_, db, mut store) = fixture().await;
    let original = claimed(&store, &db).await;
    let second = second_store(&db).await;
    let pause = Pause::new(Point::AfterCommit);
    store.commit_pause = Some(pause.clone());
    let (next, event) = advance(original.clone());
    let committed = next.clone();
    let copy = original.clone();
    let write = tokio::spawn(async move {
        store
            .commit(
                next,
                vec![event],
                copy.revision,
                Some(&copy.work.lease.as_ref().unwrap().token),
            )
            .await
    });
    pause.reached().await;
    assert_eq!(
        second
            .replay(&SubjectId::new("owner"), "accepted")
            .await
            .unwrap(),
        Some(committed.clone())
    );
    let before = snapshot(&second).await;
    let (stale, event) = advance(original.clone());
    assert_eq!(
        second
            .commit(
                stale,
                vec![event],
                original.revision,
                Some(&original.work.lease.as_ref().unwrap().token)
            )
            .await,
        Err(AccountDeletionError::Conflict)
    );
    assert_eq!(snapshot(&second).await, before);
    let events = second.events(&original.id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    pause.release();
    write.await.unwrap().unwrap();
    db.drop().await.unwrap();
}
