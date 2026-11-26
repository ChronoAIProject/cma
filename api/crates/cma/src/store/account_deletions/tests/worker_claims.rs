use super::*;
mod journal;
mod timing;

async fn snapshot(store: &MongoAccountDeletionStore) -> (Vec<Document>, Vec<Document>) {
    let rows = store
        .records
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let events = store
        .events
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    (rows, events)
}

async fn expired_transition(kind: &str) {
    let (_, db, store) = fixture().await;
    let mut record = store
        .request(&SubjectId::new("expired-owner"), "accepted", NOW)
        .await
        .unwrap();
    record
        .claim("expired-worker", NOW, Duration::from_secs(90))
        .unwrap();
    // Retained lease fixture: the caller can still validate against its old clock.
    store
        .records
        .replace_one(
            doc! {"_id":record.id.as_str()},
            record_document(&record, &[key_hash("accepted")]).unwrap(),
        )
        .await
        .unwrap();
    store
        .events
        .insert_one(
            event_document(&record.current_event(NOW, ErasureEventKind::Started), None).unwrap(),
        )
        .await
        .unwrap();
    store
        .request(&SubjectId::new("foreign-owner"), "foreign", NOW)
        .await
        .unwrap();
    let before = snapshot(&store).await;
    let lease = record.work.lease.clone().unwrap();
    let revision = record.revision;
    let events = match kind {
        "advance" => vec![record
            .advance(lease.fence, NOW, Default::default())
            .unwrap()],
        "renew" => {
            record
                .renew(lease.fence, NOW, Duration::from_secs(365 * 86400))
                .unwrap();
            vec![]
        }
        "defer" => {
            record.defer(lease.fence, LATER).unwrap();
            vec![]
        }
        _ => unreachable!(),
    };
    assert_eq!(
        store
            .commit(record, events, revision, Some(&lease.token))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(snapshot(&store).await, before);
    assert!(store
        .erasing(&SubjectId::new("expired-owner"))
        .await
        .unwrap());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_claim_cannot_advance_using_an_old_caller_clock() {
    expired_transition("advance").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_claim_cannot_renew_itself_into_the_future() {
    expired_transition("renew").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_claim_cannot_clear_its_lease_by_deferring() {
    expired_transition("defer").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_fast_caller_clock_cannot_take_over_a_live_claim() {
    let (_, db, store) = fixture().await;
    let at = database_now(&db).await;
    store
        .request(&SubjectId::new("owner"), "accepted", &at)
        .await
        .unwrap();
    let current = store
        .claim_due("current", &at, Duration::from_secs(300))
        .await
        .unwrap()
        .unwrap();
    let before = snapshot(&store).await;
    let future = &current.work.lease.as_ref().unwrap().expires_at;
    assert_eq!(
        store
            .claim_due("fast-clock", future, Duration::from_secs(90))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(snapshot(&store).await, before);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn an_already_expired_new_claim_is_never_persisted() {
    let (_, db, store) = fixture().await;
    store
        .request(&SubjectId::new("owner"), "accepted", NOW)
        .await
        .unwrap();
    let before = snapshot(&store).await;
    assert_eq!(
        store
            .claim_due("old-clock", NOW, Duration::from_secs(90))
            .await,
        Err(AccountDeletionError::StaleFence)
    );
    assert_eq!(snapshot(&store).await, before);
    db.drop().await.unwrap();
}
