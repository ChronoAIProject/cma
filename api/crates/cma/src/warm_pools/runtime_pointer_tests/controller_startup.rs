use super::*;
use crate::{startup::pointer_pool_startup, warm_pools::startup::WarmPoolStartup};
use layer3_adapter::hosted::RuntimeRoutingStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_pointer_startup_holds_ambiguous_pool_authority_without_bootstrapping_it() {
    let fixture = Fixture::new().await;
    let stale = fixture.replica().await;
    stale.seed_defaults().await;
    let old = stale
        .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
        .await;
    fixture
        .database
        .collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id": &old.id},
            doc! {"$set": {"activated_at": DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let active = fixture.replica_pinned_to('b').await;
    active.seed_defaults().await;
    let current = active
        .target_for(&fixture.plan(Lane::Agent, 'b', capacity()))
        .await;
    fixture
        .database
        .collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id": &old.id},
            doc! {"$set": {"activated_at": current.activated_at.unwrap()}},
        )
        .await
        .unwrap();
    let pointer = bind_controller(&stale, |_| panic!("startup only reads MongoDB"), true).await;
    assert!(pointer.controller.following_configured());
    assert!(pointer_pool_startup(&pointer.controller, None)
        .await
        .is_err());
    pointer.controller.register_boot().await.unwrap();
    assert!(matches!(
        pointer_pool_startup(&pointer.controller, None)
            .await
            .unwrap(),
        WarmPoolStartup::ConfigurationDrift
    ));
    assert!(pointer.pools.default_held(false));
    assert!(!pointer.pools.default_held(true));
    for lane in [ReleasedLane::Agent, ReleasedLane::Machine] {
        assert!(pointer
            .controller
            .release
            .lanes
            .load(lane)
            .await
            .unwrap()
            .is_none());
    }
    let current_after = stale.store.find(&current.id).await.unwrap().unwrap();
    assert_eq!(
        mongodb::bson::serialize_to_document(&current_after).unwrap(),
        mongodb::bson::serialize_to_document(&current).unwrap()
    );
    assert!(fixture
        .provider
        .inventory
        .lock()
        .unwrap()
        .effects
        .is_empty());
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_pointer_startup_rechecks_a_release_activated_after_preflight() {
    for published in [false, true] {
        let fixture = Fixture::new().await;
        let stale = fixture.replica().await;
        stale.seed_defaults().await;
        let old = stale
            .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
            .await;
        fixture
            .database
            .collection::<Document>("native_warm_pool_targets")
            .update_one(
                doc! {"_id": &old.id},
                doc! {"$set": {"activated_at": DateTime::from_millis(1)}},
            )
            .await
            .unwrap();
        let pointer = bind_controller(&stale, |_| panic!("startup only reads MongoDB"), true).await;
        pointer.controller.register_boot().await.unwrap();
        let (reached, resume) = stale.retirement.hold_preparation(&old.id);
        let newer = async {
            reached.await.unwrap();
            let active = fixture.replica_pinned_to('b').await;
            active.seed_defaults().await;
            for (lane, digest) in [(Lane::Agent, 'b'), (Lane::Machine, '1')] {
                let plan = fixture.plan(lane, digest, capacity());
                fixture
                    .provider
                    .configure_planned(&plan, TARGET)
                    .await
                    .unwrap();
                fixture.readiness(&plan, TARGET);
            }
            if published {
                pointer
                    .controller
                    .release
                    .lanes
                    .bootstrap(
                        layer2_core::released_runtime::ReleasedLaneRecord::bootstrap(
                            ReleasedLane::Agent,
                            image(Lane::Agent, 'b'),
                            &layer2_core::now_rfc3339(),
                        )
                        .unwrap(),
                    )
                    .await
                    .unwrap();
            }
            resume.send(()).unwrap();
        };
        let (outcome, ()) = tokio::time::timeout(DEADLINE, async {
            tokio::join!(pointer_pool_startup(&pointer.controller, None), newer)
        })
        .await
        .unwrap();
        let outcome = outcome.unwrap();
        if !published {
            assert!(matches!(outcome, WarmPoolStartup::ConfigurationDrift));
            assert!(pointer.pools.default_held(false));
            assert!(!pointer.pools.default_held(true));
            assert!(pointer
                .controller
                .release
                .lanes
                .load(ReleasedLane::Agent)
                .await
                .unwrap()
                .is_none());
            fixture.database.drop().await.unwrap();
            continue;
        }
        assert!(matches!(outcome, WarmPoolStartup::Ready));
        assert_eq!(
            pointer.durable_release(Lane::Agent).await.active,
            image(Lane::Agent, 'b')
        );
        assert_eq!(
            pointer.durable_release(Lane::Machine).await.active,
            image(Lane::Machine, '1')
        );
        let machine = pointer
            .admit(Lane::Machine, "machine-after-bootstrap")
            .await
            .unwrap();
        assert_eq!(runtime(&machine), image(Lane::Machine, '1'));
        assert_eq!(
            runtime(
                &pointer
                    .admit(Lane::Agent, "agent-after-bootstrap")
                    .await
                    .unwrap()
            ),
            image(Lane::Agent, 'b')
        );
        fixture.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn existing_machine_pointer_holds_only_a_superseded_same_image_size() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let agent = fixture.plan(Lane::Agent, 'a', capacity());
    let old_plan = fixture.plan(Lane::Machine, '1', capacity());
    for plan in [&agent, &old_plan] {
        fixture
            .provider
            .configure_planned(plan, TARGET)
            .await
            .unwrap();
        fixture.readiness(plan, TARGET);
    }
    let active = bind_controller(&original, |_| panic!("startup only reads MongoDB"), true).await;
    active.bootstrap().await.unwrap();
    let old = original.target_for(&old_plan).await;
    let new_size = SandboxCapacity {
        memory_mib: 4096,
        ..capacity()
    };
    let new_plan = fixture.plan(Lane::Machine, '1', new_size);
    let mut replacement = old.clone();
    replacement.id = "new-machine-sizing".into();
    replacement.plan = new_plan;
    replacement.requested.memory_mib = new_size.memory_mib;
    replacement.binding = None;
    replacement.activated_at = None;
    replacement.machine_plan_activated_at = None;
    replacement.lease = None;
    replacement.lease_until = DateTime::from_millis(0);
    replacement.applied_revision = 0;
    original.store.insert(replacement.clone()).await.unwrap();
    original.store.activate(&replacement.id).await.unwrap();

    let stale = fixture.replica().await;
    let pointer = bind_controller(&stale, |_| panic!("startup only reads MongoDB"), true).await;
    pointer.controller.register_boot().await.unwrap();
    let before = fixture.provider.inventory.lock().unwrap().effects.len();
    let outcome = tokio::time::timeout(DEADLINE, pointer_pool_startup(&pointer.controller, None))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, WarmPoolStartup::Ready));
    assert!(pointer.pools.default_held(true));
    assert!(!pointer.pools.default_held(false));
    let routing = crate::store::runtime_routing::MongoRuntimeRouting::open_with_default_pool_holds(
        &fixture.database,
        DEPLOYMENT,
        true,
        pointer.pools.default_holds.clone(),
    )
    .await
    .unwrap();
    assert!(routing
        .require_allocation_image(&image(Lane::Machine, '1'))
        .is_err());
    assert!(routing
        .require_allocation_image(&image(Lane::Agent, 'a'))
        .is_ok());
    assert_eq!(
        pointer.durable_release(Lane::Machine).await.active,
        image(Lane::Machine, '1')
    );
    assert_eq!(
        pointer.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'a')
    );
    {
        let inventory = fixture.provider.inventory.lock().unwrap();
        assert!(inventory.effects[before..].iter().all(|effect| !matches!(effect,
            Effect::Configure(hash, _) | Effect::Resize(hash, _) if hash == &old_plan.config_sha256)));
    }
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_pointer_startups_share_one_legacy_machine_selection() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let agent = fixture.plan(Lane::Agent, 'a', capacity());
    let machine = fixture.plan(Lane::Machine, '1', capacity());
    for plan in [&agent, &machine] {
        fixture
            .provider
            .configure_planned(plan, TARGET)
            .await
            .unwrap();
        fixture.readiness(plan, TARGET);
    }
    let active = bind_controller(&original, |_| panic!("startup only reads MongoDB"), true).await;
    active.bootstrap().await.unwrap();
    let legacy = original.target_for(&machine).await;
    fixture
        .database
        .collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id": &legacy.id},
            doc! {"$unset": {"machine_plan_activated_at": ""}},
        )
        .await
        .unwrap();
    let first = fixture.replica().await;
    let second = fixture.replica().await;
    let a = bind_controller(&first, |_| panic!("startup only reads MongoDB"), true).await;
    let b = bind_controller(&second, |_| panic!("startup only reads MongoDB"), true).await;
    a.controller.register_boot().await.unwrap();
    b.controller.register_boot().await.unwrap();
    let (one, two) = tokio::time::timeout(DEADLINE, async {
        tokio::join!(
            pointer_pool_startup(&a.controller, None),
            pointer_pool_startup(&b.controller, None)
        )
    })
    .await
    .unwrap();
    assert!(matches!(one.unwrap(), WarmPoolStartup::Ready));
    assert!(matches!(two.unwrap(), WarmPoolStartup::Ready));
    assert!(!a.pools.default_held(true));
    assert!(!b.pools.default_held(true));
    let selected = first.target_for(&machine).await;
    assert_eq!(selected.id, legacy.id);
    assert_eq!(selected.activated_at, legacy.activated_at);
    assert!(selected.machine_plan_activated_at.is_some());
    assert_eq!(
        second.target_for(&machine).await.machine_plan_activated_at,
        selected.machine_plan_activated_at
    );
    fixture.database.drop().await.unwrap();
}
