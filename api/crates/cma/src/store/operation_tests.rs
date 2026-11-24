use futures_util::TryStreamExt as _;
use layer2_core::SubjectId;

use super::*;

fn claim(key: &str, fingerprint: u8, lease_expires_at: SystemTime) -> OperationClaim {
    OperationClaim::new(
        SubjectId("subject-a".into()),
        "create-resource",
        key,
        [fingerprint; 32],
        lease_expires_at,
        Duration::from_secs(86_400),
    )
}

fn claim_with_lease(
    key: &str,
    fingerprint: u8,
    lease_expires_at: SystemTime,
    outcome_retention: Duration,
) -> OperationClaim {
    OperationClaim::new(
        SubjectId("subject-a".into()),
        "create-resource",
        key,
        [fingerprint; 32],
        lease_expires_at,
        outcome_retention,
    )
}

#[test]
fn expiry_is_limited_to_completed_outcomes() {
    let expiry = expiry_index();
    assert_eq!(expiry.keys, doc! { "expires_at": 1 });
    let options = expiry.options.unwrap();
    assert_eq!(options.expire_after, Some(Duration::ZERO));
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "outcome": { "$type": "binData" } })
    );
}

#[test]
fn the_compound_identity_and_fingerprint_round_trip() {
    let held = claim(
        "retry-a",
        7,
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
    );
    let stored = to_doc(&held);
    assert_eq!(stored.document_schema_version, DOCUMENT_SCHEMA_VERSION);
    assert_eq!(stored.id, operation_id(&held));
    assert_eq!(stored.claim_id.as_deref(), Some(held.claim_id()));
    assert_eq!(stored.expires_at, None);
    assert_eq!(fingerprint_bytes(&stored).unwrap(), held.fingerprint());
}

#[test]
fn recovery_fence_requires_the_exact_unfinished_expired_lease() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let held = claim_with_lease(
        "stale",
        4,
        now - Duration::from_secs(1),
        Duration::from_secs(86_400),
    );
    let filter = recovery_filter(&held, now);
    assert_eq!(filter.get("_id"), Some(&id_bson(&held)));
    assert_eq!(
        filter.get("fingerprint"),
        Some(&Bson::Binary(binary(held.fingerprint().to_vec())))
    );
    assert_eq!(filter.get("outcome"), Some(&Bson::Null));
    assert!(filter.contains_key("$or"));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_reserves_once_replays_and_reuses_expired_keys() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_operation_test_{}", uuid::Uuid::new_v4().simple());
    let first = MongoOperationStore::open(&client, &database_name)
        .await
        .unwrap();
    let second = MongoOperationStore::open(&client, &database_name)
        .await
        .unwrap();

    let indexes = client
        .database(&database_name)
        .collection::<mongodb::bson::Document>(OPERATIONS)
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let expiry = indexes
        .into_iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(EXPIRY_INDEX)
        })
        .expect("named operation expiry index exists");
    assert_eq!(expiry.keys, doc! { "expires_at": 1 });
    assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));

    let future = SystemTime::now() + Duration::from_secs(120);
    let held = claim("held", 1, future);
    let (left, right) = tokio::join!(first.reserve(&held), second.reserve(&held));
    let reservations = [left.unwrap(), right.unwrap()];
    assert_eq!(
        reservations
            .iter()
            .filter(|result| matches!(result, OperationReservation::Acquired))
            .count(),
        1
    );
    assert_eq!(
        reservations
            .iter()
            .filter(|result| matches!(result, OperationReservation::Pending))
            .count(),
        1
    );

    first.complete(&held, b"created".to_vec()).await.unwrap();
    assert_eq!(
        second.reserve(&held).await.unwrap(),
        OperationReservation::Complete(b"created".to_vec())
    );
    assert_eq!(
        second.reserve(&claim("held", 2, future)).await.unwrap(),
        OperationReservation::FingerprintMismatch
    );

    let expired = claim_with_lease("expired", 3, future, Duration::ZERO);
    assert_eq!(
        first.reserve(&expired).await.unwrap(),
        OperationReservation::Acquired
    );
    first
        .complete(&expired, b"expired outcome".to_vec())
        .await
        .unwrap();
    let reusable = claim("expired", 3, future);
    assert_eq!(
        second.reserve(&reusable).await.unwrap(),
        OperationReservation::Acquired
    );
    assert!(first
        .complete(&expired, b"stale outcome".to_vec())
        .await
        .is_err());
    assert_eq!(
        second.reserve(&reusable).await.unwrap(),
        OperationReservation::Pending
    );
    second
        .complete(&reusable, b"current outcome".to_vec())
        .await
        .unwrap();

    let abandoned = claim_with_lease(
        "abandoned",
        5,
        SystemTime::UNIX_EPOCH,
        Duration::from_secs(86_400),
    );
    first
        .operations
        .insert_one(to_doc(&abandoned))
        .await
        .unwrap();
    assert_eq!(
        second.reserve(&abandoned).await.unwrap(),
        OperationReservation::RecoveryRequired
    );
    assert_eq!(
        first.reserve(&abandoned).await.unwrap(),
        OperationReservation::RecoveryRequired
    );
    first
        .complete(&abandoned, b"reconciled".to_vec())
        .await
        .unwrap();
    assert_eq!(
        second.reserve(&abandoned).await.unwrap(),
        OperationReservation::Complete(b"reconciled".to_vec())
    );

    let database = client.database(&database_name);
    crate::worker_supervision::startup_recovery_with_owned_completion(database.clone(), first)
        .await;
    database.drop().await.unwrap();
}

#[path = "operation_tests/independent_claim_owner.rs"]
mod independent_claim_owner;
