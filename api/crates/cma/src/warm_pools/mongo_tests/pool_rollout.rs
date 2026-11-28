use super::*;

async fn fixture() -> (mongodb::Database, Arc<MongoNativeWarmPools>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!("pool_rollout_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(MongoNativeWarmPools::open(&db, "test").await.unwrap());
    (db, store)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fixed_rollout_keeps_both_incumbents_until_each_exact_candidate_is_ready() {
    let (db, store) = fixture().await;
    let planner = resize::Planner::default();
    let incumbent = resize::pinned("standard", 'a');
    let next = resize::pinned("standard", 'b');
    let machine = resize::pinned("machine", 'c');
    let next_machine = resize::pinned("machine", 'd');
    let prepare = [Some(next.clone()), Some(next_machine.clone())];
    let old = resize::deployment(
        store.clone(),
        planner.clone(),
        &incumbent,
        Some((&machine, 1)),
    )
    .with_fixed_pool_prewarm(prepare.clone());
    let new = resize::deployment(
        store.clone(),
        planner.clone(),
        &next,
        Some((&next_machine, 1)),
    )
    .with_fixed_pool_prewarm(prepare);
    old.ensure_fixed_defaults().await.unwrap();
    let a = old.active_default_target(&incumbent, false).await.unwrap();
    let m = old.active_default_target(&machine, true).await.unwrap();
    assert!(a.activated_at.is_some() && m.machine_plan_activated_at.is_some());
    assert!(matches!(
        new.ensure_fixed_defaults().await,
        Err(WarmPoolError::Conflict)
    ));

    let mut candidates = Vec::new();
    for (image, machine) in [(&next, false), (&next_machine, true)] {
        let target = new.active_default_target(image, machine).await.unwrap();
        assert!(target.activated_at.is_none());
        let claimed = store.claim_target(&target.id).await.unwrap().unwrap();
        assert!(old.matches_fixed_preparation(&claimed).await.unwrap());
        let binding = resize::Planner::binding(&target.plan);
        store.authorize_provider_create(&claimed).await.unwrap();
        store.bind(&claimed, &binding).await.unwrap();
        store.finish(&claimed, true).await.unwrap();
        planner.inventory.lock().unwrap().push(NativePoolStatus {
            binding,
            desired_replicas: 1,
            ready_replicas: 0,
            observed: true,
        });
        candidates.push(target.id);
    }
    assert!(matches!(
        new.ensure_fixed_defaults().await,
        Err(WarmPoolError::Unavailable)
    ));
    planner.inventory.lock().unwrap()[0].ready_replicas = 1;
    assert!(matches!(
        new.ensure_fixed_defaults().await,
        Err(WarmPoolError::Unavailable)
    ));
    for id in &candidates {
        assert!(store
            .find(id)
            .await
            .unwrap()
            .unwrap()
            .activated_at
            .is_none());
    }
    planner.inventory.lock().unwrap()[1].ready_replicas = 1;
    for _ in 0..2 {
        new.ensure_fixed_defaults().await.unwrap();
    }
    assert!(store
        .find(&candidates[0])
        .await
        .unwrap()
        .unwrap()
        .activated_at
        .is_some());
    assert!(store
        .find(&candidates[1])
        .await
        .unwrap()
        .unwrap()
        .machine_plan_activated_at
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unbound_retirement_fences_late_provider_creation_and_keeps_uncertain_rows() {
    let (db, store) = fixture().await;
    let planner = resize::Planner::default();
    planner.retirement_absent.store(true, Ordering::SeqCst);
    let active = resize::pinned("standard", 'b');
    let epoch = uuid::Uuid::new_v4();
    let pools = resize::deployment(store.clone(), planner.clone(), &active, None)
        .with_unbound_retirement_fencing(Some(epoch));
    pools.ensure_fixed_defaults().await.unwrap();
    let current = pools.active_default_target(&active, false).await.unwrap();
    let rows = db.collection::<Document>("native_warm_pool_targets");
    rows.update_one(
        doc! {"_id": current.id},
        doc! {"$set": {"activated_at": DateTime::from_millis(3)}},
    )
    .await
    .unwrap();

    for (digit, dispatched, fenced, legacy) in [
        ('a', false, false, false),
        ('c', false, true, false),
        ('d', true, true, false),
        ('e', false, true, true),
    ] {
        if digit == 'c' {
            db.collection::<Document>("platform_settings")
                .insert_one(doc! {
                    "_id": "native_pool_create_cutover:test", "deployment": "test",
                    "document_schema_version": 1_i64,
                    "epoch": epoch.to_string(), "legacy_creates_settled": true, "state": "fenced",
                })
                .await
                .unwrap();
        }
        let old = pools
            .prepare_released_image(&resize::pinned("standard", digit), false)
            .await
            .unwrap();
        if legacy {
            rows.update_one(
                doc! {"_id": &old.id},
                doc! {"$unset": {"provider_create_never_dispatched": ""}},
            )
            .await
            .unwrap();
        }
        let stale = store.claim_target(&old.id).await.unwrap().unwrap();
        if dispatched {
            store.authorize_provider_create(&stale).await.unwrap();
        }
        rows.update_one(
            doc! {"_id": &old.id},
            doc! {"$set": {"lease_until": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
        let owned = store.claim_retirement(&old.id).await.unwrap().unwrap();
        assert!(store.authorize_provider_create(&stale).await.is_err());
        pools.retire_claimed(&planner, owned).await.unwrap();
        let target = store.find(&old.id).await.unwrap().unwrap();
        assert_eq!(target.retired, fenced && !dispatched);
        assert_eq!(target.target_replicas, 0);
        if dispatched {
            assert!(target.error.is_some());
        }
    }
    assert!(planner.retirement_calls.lock().unwrap().is_empty());
    db.drop().await.unwrap();
}
