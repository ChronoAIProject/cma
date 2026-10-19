use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;

async fn fixture() -> (Database, MongoNativeWarmPools, PoolTarget, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_pool_retirement_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = MongoNativeWarmPools::open(&db, "test").await.unwrap();
    let mut old = crate::warm_pools::tests::target();
    old.activated_at = Some(DateTime::from_millis(1));
    store.insert(old.clone()).await.unwrap();
    let mut current = old.clone();
    current.id = "current".into();
    current.plan.config_sha256 = "b".repeat(64);
    current.plan.spec.image.uri = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    current.activated_at = Some(DateTime::from_millis(2));
    let image = current.plan.spec.image.uri.clone();
    store.insert(current).await.unwrap();
    (db, store, old, image)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn activation_is_immutable_and_old_process_cannot_retire_new_release() {
    let (db, store, old, image) = fixture().await;
    assert!(matches!(
        store.activate(&old.id).await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store.find(&old.id).await.unwrap().unwrap().activated_at,
        old.activated_at
    );
    assert_eq!(
        store.retirement_homes(&old, &image).await.unwrap(),
        Some(vec![])
    );
    let current = store.find("current").await.unwrap().unwrap();
    assert!(store
        .retirement_homes(&current, &old.plan.spec.image.uri)
        .await
        .unwrap()
        .is_none());
    let collection = db.collection::<Document>(COLLECTION);
    collection
        .update_one(
            doc! {"_id":"current"},
            doc! {"$set":{"activated_at":DateTime::now()}},
        )
        .await
        .unwrap();
    assert!(
        store
            .retirement_homes(&old, &image)
            .await
            .unwrap()
            .is_none(),
        "rollout grace must apply"
    );
    collection
        .update_one(
            doc! {"_id":"current"},
            doc! {"$set":{"activated_at":old.activated_at}},
        )
        .await
        .unwrap();
    assert!(
        store
            .retirement_homes(&old, &image)
            .await
            .unwrap()
            .is_none(),
        "ambiguous release order must defer"
    );
    assert!(matches!(
        store.activate("current").await,
        Err(WarmPoolError::Conflict)
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn changed_plan_cannot_promote_a_superseded_runtime_image() {
    let (db, store, old, _) = fixture().await;
    let mut changed = old.clone();
    changed.id = "changed-old-policy".into();
    changed.plan.config_sha256 = "c".repeat(64);
    changed.activated_at = None;
    store.insert(changed.clone()).await.unwrap();
    assert!(matches!(
        store.activate(&changed.id).await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store.find(&changed.id).await.unwrap().unwrap().activated_at,
        old.activated_at,
        "changed policy must not reset the runtime image's release order"
    );
    let mut current = store.find("current").await.unwrap().unwrap();
    let released_at = current.activated_at;
    current.id = "changed-current-policy".into();
    current.plan.config_sha256 = "d".repeat(64);
    current.activated_at = None;
    store.insert(current.clone()).await.unwrap();
    store.activate(&current.id).await.unwrap();
    assert_eq!(
        store.find(&current.id).await.unwrap().unwrap().activated_at,
        released_at
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fresh_scan_blocks_old_runtime_and_late_allocations_and_corrupt_authority() {
    let (db, store, old, image) = fixture().await;
    let sandboxes = db.collection::<Document>("sandboxes");
    let mut sandbox = active_sandbox("retirement-test");
    sandbox.runtime.as_mut().unwrap().runtime_version = old.plan.spec.image.uri.clone();
    let doc = crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap();
    sandboxes.insert_one(doc).await.unwrap();
    assert!(store
        .retirement_homes(&old, &image)
        .await
        .unwrap()
        .is_none());
    sandboxes.delete_many(doc! {}).await.unwrap();
    assert_eq!(
        store.retirement_homes(&old, &image).await.unwrap(),
        Some(vec![])
    );
    // A reservation inserted between pre-marker scan and post-marker scan is
    // visible even before it has selected or stored a provider Claim.
    sandbox.status = SandboxStatus::Provisioning;
    sandbox.runtime = None;
    let doc = crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap();
    sandboxes.insert_one(doc).await.unwrap();
    assert!(store
        .retirement_homes(&old, &image)
        .await
        .unwrap()
        .is_none());
    sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":{"revision":99_i64}},
        )
        .await
        .unwrap();
    assert!(store.retirement_homes(&old, &image).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_retains_original_authority_and_requires_current_lease_to_delete() {
    let (db, store, old, image) = fixture().await;
    let retired = store.begin_retirement(&old).await.unwrap();
    assert!(retired.retiring);
    assert_eq!(retired.target_replicas, 0);
    assert_eq!(retired.plan, old.plan);
    assert_eq!(retired.revision, old.revision + 1);
    assert!(matches!(
        store.remove_retired(&old).await,
        Err(WarmPoolError::Conflict)
    ));
    assert!(matches!(
        store
            .resize(
                &retired.id,
                &ResizeWarmPoolBody {
                    expected_revision: retired.revision as u64,
                    target_replicas: 3
                }
            )
            .await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store.retirement_homes(&retired, &image).await.unwrap(),
        Some(vec![])
    );
    db.collection::<Document>(COLLECTION)
        .update_one(
            doc! {"_id":&retired.id},
            doc! {"$set":{"lease":"another-worker"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.remove_retired(&retired).await,
        Err(WarmPoolError::Conflict)
    ));
    assert!(store.retirement_homes(&retired, &image).await.is_err());
    let next = store.find(&retired.id).await.unwrap().unwrap();
    store.remove_retired(&next).await.unwrap();
    let tombstone = store.find(&retired.id).await.unwrap().unwrap();
    assert!(tombstone.retired);
    assert_eq!(tombstone.activated_at, old.activated_at);
    assert_eq!(store.list().await.unwrap().len(), 1);
    assert!(store.claim().await.unwrap().is_none());
    let replayed = store.insert(old.clone()).await.unwrap();
    assert!(
        replayed.retired,
        "old startup must recover the tombstone, never insert a new target"
    );
    assert!(matches!(
        store.activate(&old.id).await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(
        store.find(&old.id).await.unwrap().unwrap().activated_at,
        old.activated_at
    );
    assert!(store.find("current").await.unwrap().is_some());
    db.drop().await.unwrap();
}
