//! Disposable-Mongo checks for independent claim generations sharing one request identity.
use super::*;

fn malformed(result: Result<(), StoreError>) {
    assert!(
        matches!(result, Err(StoreError::Malformed(_))),
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_independent_claim_owners_cannot_finish_or_replace_each_other() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires a disposable CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_claim_owner_test_{}", uuid::Uuid::new_v4().simple());
    let first = MongoOperationStore::open(&client, &database_name)
        .await
        .unwrap();
    let second = MongoOperationStore::open(&client, &database_name)
        .await
        .unwrap();
    let future = SystemTime::now() + Duration::from_secs(120);

    // Separate requests create separate claim IDs despite the same public request identity.
    let left = claim("independent", 1, future);
    let right = claim("independent", 1, future);
    assert_ne!(left.claim_id(), right.claim_id());
    let (a, b) = tokio::join!(first.reserve(&left), second.reserve(&right));
    let (owner, observer) = match (a.unwrap(), b.unwrap()) {
        (OperationReservation::Acquired, OperationReservation::Pending) => (&left, &right),
        (OperationReservation::Pending, OperationReservation::Acquired) => (&right, &left),
        outcomes => panic!("exactly one independent request must acquire: {outcomes:?}"),
    };
    let held = first.held(owner).await.unwrap().unwrap();
    assert_eq!(held.claim_id.as_deref(), Some(owner.claim_id()));
    assert!(held.outcome.is_none());
    malformed(second.complete(observer, b"not the owner".to_vec()).await);
    assert!(first.held(owner).await.unwrap().unwrap().outcome.is_none());

    // A foreign completion racing the real owner's completion cannot replace its bytes.
    let (owned, foreign) = tokio::join!(
        first.complete(owner, b"accepted".to_vec()),
        second.complete(observer, b"foreign".to_vec())
    );
    owned.unwrap();
    malformed(foreign);
    assert_eq!(
        second.reserve(observer).await.unwrap(),
        OperationReservation::Complete(b"accepted".to_vec())
    );
    malformed(second.complete(observer, b"accepted".to_vec()).await);
    first.complete(owner, b"accepted".to_vec()).await.unwrap();
    malformed(first.complete(owner, b"changed".to_vec()).await);
    assert_eq!(
        first
            .held(owner)
            .await
            .unwrap()
            .unwrap()
            .outcome
            .unwrap()
            .bytes,
        b"accepted"
    );

    // Expired unfinished work remains owned by its original generation after recovery fencing.
    let original = claim("unfinished", 2, SystemTime::UNIX_EPOCH);
    assert_eq!(
        first.reserve(&original).await.unwrap(),
        OperationReservation::Acquired
    );
    let recovery_observer = claim("unfinished", 2, future);
    assert_ne!(original.claim_id(), recovery_observer.claim_id());
    assert_eq!(
        second.reserve(&recovery_observer).await.unwrap(),
        OperationReservation::RecoveryRequired
    );
    malformed(
        second
            .complete(&recovery_observer, b"invented".to_vec())
            .await,
    );
    let held = first.held(&original).await.unwrap().unwrap();
    assert_eq!(held.claim_id.as_deref(), Some(original.claim_id()));
    assert!(held.recovery_required);
    assert!(held.outcome.is_none());
    first
        .complete(&original, b"original exact result".to_vec())
        .await
        .unwrap();
    assert_eq!(
        second.reserve(&recovery_observer).await.unwrap(),
        OperationReservation::Complete(b"original exact result".to_vec())
    );
    malformed(
        second
            .complete(&recovery_observer, b"original exact result".to_vec())
            .await,
    );

    // After a completed outcome expires, the old generation cannot complete a new generation.
    let expired = claim_with_lease("replacement", 3, future, Duration::ZERO);
    assert_eq!(
        first.reserve(&expired).await.unwrap(),
        OperationReservation::Acquired
    );
    first.complete(&expired, b"old".to_vec()).await.unwrap();
    let replacement = claim("replacement", 3, future);
    assert_ne!(expired.claim_id(), replacement.claim_id());
    assert_eq!(
        second.reserve(&replacement).await.unwrap(),
        OperationReservation::Acquired
    );
    malformed(first.complete(&expired, b"old".to_vec()).await);
    let held = second.held(&replacement).await.unwrap().unwrap();
    assert_eq!(held.claim_id.as_deref(), Some(replacement.claim_id()));
    assert!(held.outcome.is_none());
    assert_eq!(
        first.reserve(&replacement).await.unwrap(),
        OperationReservation::Pending
    );
    second
        .complete(&replacement, b"new".to_vec())
        .await
        .unwrap();
    malformed(first.complete(&expired, b"new".to_vec()).await);
    assert_eq!(
        first.reserve(&expired).await.unwrap(),
        OperationReservation::Complete(b"new".to_vec())
    );
    assert_eq!(
        second
            .held(&replacement)
            .await
            .unwrap()
            .unwrap()
            .outcome
            .unwrap()
            .bytes,
        b"new"
    );
    client.database(&database_name).drop().await.unwrap();
}
