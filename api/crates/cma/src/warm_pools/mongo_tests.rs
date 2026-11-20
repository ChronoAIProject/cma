use super::*;
use crate::store::native_warm_pools::MongoNativeWarmPools;
use mongodb::bson::{doc, DateTime, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_warm_workers_keep_shared_target_and_original_claim_on_takeover() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let a = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let b = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!("warm_workers_{}", uuid::Uuid::new_v4().simple());
    let db = a.database(&name);
    let first = MongoNativeWarmPools::open(&db, "test").await.unwrap();
    let second = MongoNativeWarmPools::open(&b.database(&name), "test")
        .await
        .unwrap();
    let mut original = target();
    original.lease = None;
    original.lease_until = DateTime::from_millis(0);
    first.insert(original.clone()).await.unwrap();
    second.insert(original.clone()).await.unwrap();
    let (left, right) = tokio::join!(first.claim(), second.claim());
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_ne!(left.is_some(), right.is_some());
    let stale = left.or(right).unwrap();
    db.collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id":&original.id},
            doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    second
        .resize(
            &original.id,
            &ResizeWarmPoolBody {
                expected_revision: 1,
                target_replicas: 2,
            },
        )
        .await
        .unwrap();
    let current = second.claim().await.unwrap().unwrap();
    assert_ne!(current.lease, stale.lease);
    assert_eq!(current.revision, 2);
    assert_eq!(current.target_replicas, 2);
    assert!(first.finish(&stale, true).await.is_err());
    second.finish(&current, true).await.unwrap();
    assert_eq!(first.list().await.unwrap().len(), 1);
    assert_eq!(
        first
            .find(&original.id)
            .await
            .unwrap()
            .unwrap()
            .applied_revision,
        2
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn durable_target_replay_unique_plan_leases_revision_and_uid_binding() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri)
        .await
        .expect("test Mongo configuration");
    let db = client.database(&format!(
        "cma_native_pool_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = MongoNativeWarmPools::open(&db, "test").await.unwrap();
    let mut original = target();
    original.lease = None;
    original.lease_until = DateTime::from_millis(0);
    let saved = store.insert(original.clone()).await.unwrap();
    assert_eq!(store.insert(original.clone()).await.unwrap().id, saved.id);
    let mut duplicate = original.clone();
    duplicate.id = "wp_other-key".into();
    assert!(matches!(
        store.insert(duplicate).await,
        Err(WarmPoolError::Conflict)
    ));
    assert!(store
        .claim_target("missing-target")
        .await
        .unwrap()
        .is_none());
    let leased = store.claim_target(&saved.id).await.unwrap().unwrap();
    assert!(store.claim_target(&saved.id).await.unwrap().is_none());
    // Retirement claims by id share the same lease fence.
    assert!(store.claim_retirement(&saved.id).await.unwrap().is_none());
    assert!(store.claim().await.unwrap().is_none());
    assert!(matches!(
        store
            .resize(
                &saved.id,
                &ResizeWarmPoolBody {
                    expected_revision: 1,
                    target_replicas: 0
                }
            )
            .await,
        Err(WarmPoolError::Conflict)
    ));
    let provider = Provider {
        calls: Arc::default(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let binding = provider.binding();
    store.bind(&leased, &binding).await.unwrap();
    let collection = db.collection::<Document>("native_warm_pool_targets");
    collection
        .update_one(
            doc! {"_id":&saved.id},
            doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let replacement_lease = store.claim().await.unwrap().unwrap();
    assert_ne!(replacement_lease.lease, leased.lease);
    assert_eq!(replacement_lease.binding.as_ref(), Some(&binding));
    assert!(matches!(
        store.finish(&leased, true).await,
        Err(WarmPoolError::Conflict)
    ));
    store.finish(&replacement_lease, true).await.unwrap();
    let resized = store
        .resize(
            &saved.id,
            &ResizeWarmPoolBody {
                expected_revision: 1,
                target_replicas: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(resized.revision, 2);
    assert_eq!(resized.target_replicas, 0);
    assert_eq!(resized.binding.as_ref(), Some(&binding));
    assert_eq!(resized.plan, original.plan);
    assert!(matches!(
        store
            .resize(
                &saved.id,
                &ResizeWarmPoolBody {
                    expected_revision: 1,
                    target_replicas: 9
                }
            )
            .await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store
            .find(&saved.id)
            .await
            .unwrap()
            .unwrap()
            .target_replicas,
        0
    );
    let another_deployment = MongoNativeWarmPools::open(&db, "another-deployment")
        .await
        .unwrap();
    assert!(another_deployment.find(&saved.id).await.unwrap().is_none());
    assert!(another_deployment.list().await.unwrap().is_empty());
    // Historical false acknowledgements do not suppress periodic reconciliation.
    collection
        .update_one(
            doc! {"_id":&saved.id},
            doc! {"$set":{
                "applied_revision":resized.revision,"reconcile_after":DateTime::from_millis(0)
            }},
        )
        .await
        .unwrap();
    let retry = store.claim().await.unwrap().unwrap();
    assert_eq!(retry.target_replicas, 0);
    assert_eq!(retry.applied_revision, retry.revision);
    collection
        .update_one(
            doc! {"_id":&saved.id},
            doc! {"$set":{"document_schema_version":2_i64}},
        )
        .await
        .unwrap();
    assert!(MongoNativeWarmPools::open(&db, "test").await.is_err());
    db.drop().await.unwrap();
}
