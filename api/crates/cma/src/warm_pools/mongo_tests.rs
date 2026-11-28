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
            false,
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
                },
                false
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
            false,
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
                },
                false
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn operator_machine_count_survives_retirement_and_concurrent_release_seeding() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let a = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let b = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_machine_carry_{}", uuid::Uuid::new_v4().simple());
    let db = a.database(&name);
    let left = Arc::new(MongoNativeWarmPools::open(&db, "test").await.unwrap());
    let right = Arc::new(
        MongoNativeWarmPools::open(&b.database(&name), "test")
            .await
            .unwrap(),
    );
    let agent = resize::pinned("agent", 'a');
    let release = |store: &Arc<MongoNativeWarmPools>, machine: char| {
        resize::deployment(
            store.clone(),
            resize::Planner::default(),
            &agent,
            Some((&resize::pinned("omarchy", machine), 0)),
        )
    };
    let machine_pool = |rows: Vec<PoolTarget>, machine: char| {
        let image = resize::pinned("omarchy", machine);
        resize::only(
            rows.into_iter()
                .filter(|row| row.machine && row.plan.spec.image.uri == image)
                .collect(),
        )
    };
    let first = release(&left, '1');
    first.ensure_default().await.unwrap();
    let rows = left.list().await.unwrap();
    let default = rows.iter().find(|row| !row.machine).unwrap().clone();
    let machine = machine_pool(rows, '1');
    assert_eq!(machine.target_replicas, 0);
    assert_eq!(left.latest_machine_operator_replicas().await.unwrap(), None);
    // An agent pool never records a machine decision, even when asked to.
    let refused = ResizeWarmPoolBody {
        expected_revision: 1,
        target_replicas: 6,
    };
    assert!(matches!(
        left.resize(&default.id, &refused, true).await,
        Err(WarmPoolError::Conflict)
    ));
    resize::resize_to(&first, &default, 6).await;
    resize::resize_to(&first, &machine, 2).await;
    let choice = left.latest_machine_operator_replicas().await.unwrap();
    assert_eq!(choice.map(|choice| choice.replicas), Some(2));

    // Retirement zeroes the count; the tombstone keeps the decision.
    let leased = left.claim_retirement(&machine.id).await.unwrap().unwrap();
    let retiring = left.begin_retirement(&leased).await.unwrap();
    assert_eq!(retiring.target_replicas, 0);
    left.remove_retired(&retiring).await.unwrap();
    assert!(left.find(&machine.id).await.unwrap().unwrap().retired);
    assert_eq!(
        left.latest_machine_operator_replicas().await.unwrap(),
        choice
    );

    // Two pods seed the next machine release at once over separate clients.
    let (one, two) = (release(&left, '2'), release(&right, '2'));
    let (one, two) = tokio::join!(one.ensure_default(), two.ensure_default());
    one.unwrap();
    two.unwrap();
    let promoted = machine_pool(right.list().await.unwrap(), '2');
    assert_eq!(promoted.target_replicas, 2);
    assert_eq!(promoted.revision, 1);
    assert_eq!(promoted.operator_replicas, choice);

    // Zero is an operator choice too, and is carried.
    resize::resize_to(&release(&right, '2'), &promoted, 0).await;
    release(&left, '3').ensure_default().await.unwrap();
    assert_eq!(
        machine_pool(left.list().await.unwrap(), '3').target_replicas,
        0
    );
    let agent_pool = left.find(&default.id).await.unwrap().unwrap();
    assert_eq!(agent_pool.target_replicas, 6);
    assert_eq!(agent_pool.operator_replicas, None);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn quarantine_defers_startup_retirement_without_masking_unresolved_effects() {
    use crate::{
        startup::Admission, store::native_warm_pools::quarantined_retirement_fixture,
        warm_pools::startup::WarmPoolStartup,
    };
    use resize::RetirementCall;
    use tokio::time::Instant;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "warm_quarantine_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(MongoNativeWarmPools::open(&db, "test").await.unwrap());
    let planner = resize::Planner::default();
    let (marked, repaired, old, image, home) = quarantined_retirement_fixture();

    let pools = resize::deployment(store.clone(), planner.clone(), &image, None)
        .with_empty_pool_startup(true);
    pools.ensure_default().await.unwrap();
    let current = resize::only(store.list().await.unwrap());
    let binding = resize::Planner::binding(&current.plan);
    let targets = db.collection::<Document>("native_warm_pool_targets");
    targets
        .update_one(
            doc! {"_id": &current.id},
            doc! {"$set": {
                "activated_at": DateTime::from_millis(2),
                "applied_revision": current.revision,
                "binding": mongodb::bson::serialize_to_bson(&binding).unwrap(),
                "reconcile_after": DateTime::from_millis(i64::MAX),
            }},
        )
        .await
        .unwrap();
    planner.inventory.lock().unwrap().push(NativePoolStatus {
        binding,
        desired_replicas: current.target_replicas,
        ready_replicas: 0,
        observed: true,
    });
    store.insert(old.clone()).await.unwrap();
    let sandboxes = db.collection::<Document>("sandboxes");
    let sandbox_id = doc! {"_id": marked.get("_id").unwrap()};
    sandboxes.insert_one(&marked).await.unwrap();

    let boot = || async {
        pools
            .await_default_ready_by(
                Some(&Admission::new()),
                None,
                Instant::now() + Duration::from_secs(2),
            )
            .await
    };
    assert!(matches!(
        boot().await.unwrap(),
        WarmPoolStartup::CapacityDeferred(proof) if proof.desired() == current.target_replicas
    ));
    let held = store.find(&old.id).await.unwrap().unwrap();
    assert!(held.reconcile_after > old.reconcile_after);
    assert!(held.lease.is_none() && held.error.is_none() && !held.retiring);
    assert_eq!(held.revision, old.revision);
    assert_eq!(held.target_replicas, old.target_replicas);
    assert_eq!(held.binding, old.binding);
    assert!(planner.retirement_calls.lock().unwrap().is_empty());

    targets
        .update_one(
            doc! {"_id": &old.id},
            doc! {"$set": {
                "error": "previous provider effect not confirmed",
                "reconcile_after": DateTime::from_millis(0),
            }},
        )
        .await
        .unwrap();
    assert!(matches!(boot().await, Err(WarmPoolError::Unavailable)));
    let unresolved = store.find(&old.id).await.unwrap().unwrap();
    assert_eq!(
        unresolved.error.as_deref(),
        Some("previous provider effect not confirmed")
    );
    assert!(unresolved.reconcile_after > DateTime::from_millis(0));
    assert!(planner.retirement_calls.lock().unwrap().is_empty());
    let retained = sandboxes
        .find_one(sandbox_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.to_vec().unwrap(), marked.to_vec().unwrap());

    sandboxes
        .replace_one(sandbox_id.clone(), &repaired)
        .await
        .unwrap();
    targets
        .update_one(
            doc! {"_id": &old.id},
            doc! {"$set": {"reconcile_after": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(matches!(
        boot().await.unwrap(),
        WarmPoolStartup::CapacityDeferred(_)
    ));
    let retiring = store.find(&old.id).await.unwrap().unwrap();
    assert!(retiring.retiring && retiring.error.is_none() && retiring.lease.is_none());
    assert_eq!(retiring.target_replicas, 0);
    assert_eq!(retiring.applied_revision, retiring.revision);
    let uid = old.binding.unwrap().uid;
    assert_eq!(
        *planner.retirement_calls.lock().unwrap(),
        [
            RetirementCall::Begin(uid.clone()),
            RetirementCall::CheckDefinitions(vec![home.clone()]),
            RetirementCall::Drain(
                uid,
                NativePoolDrain {
                    homes: vec![home],
                    claims: vec![]
                }
            ),
        ]
    );
    assert_eq!(
        sandboxes
            .find_one(sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .to_vec()
            .unwrap(),
        repaired.to_vec().unwrap()
    );
    db.drop().await.unwrap();
}
