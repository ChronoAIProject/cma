use super::*;
use crate::{startup::pointer_pool_startup, warm_pools::startup::WarmPoolStartup};

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
        let (reached, resume) = stale.retirement.hold_activation(&old.id);
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
async fn first_pointer_publication_orders_fixed_activation_after_target_selection() {
    for fixed_first in [true, false] {
        let fixture = Fixture::new().await;
        let original = fixture.replica().await;
        original.seed_defaults().await;
        let agent = fixture.plan(Lane::Agent, 'a', capacity());
        fixture
            .provider
            .configure_planned(&agent, TARGET)
            .await
            .unwrap();
        fixture.readiness(&agent, TARGET);
        let old = original.target_for(&agent).await;
        fixture
            .database
            .collection::<Document>("native_warm_pool_targets")
            .update_one(
                doc! {"_id": &old.id},
                doc! {"$set": {"activated_at": DateTime::from_millis(1)}},
            )
            .await
            .unwrap();
        let pointer =
            bind_controller(&original, |_| panic!("startup only reads MongoDB"), true).await;
        let legacy_store = crate::store::MongoSandboxStore::open(
            original.database.client(),
            original.database.name(),
        )
        .await
        .unwrap()
        .with_managed_runtime_version(agent.spec.image.uri.clone());
        let legacy = lifecycle::Lifecycle::with_store(
            &original,
            pointer.controller.release.clone(),
            agent.spec.image.uri.clone(),
            image(Lane::Machine, '1'),
            legacy_store,
        )
        .await
        .unwrap();
        let retained = legacy.admit(Lane::Agent, "existing-home").await.unwrap();
        assert!(retained.runtime.is_some() && retained.lifecycle.home.is_some());
        pointer.controller.register_boot().await.unwrap();
        let sibling = fixture.replica_pinned_to('b').await;
        let (selected, resume) = original.retirement.hold_selection(&old.id);
        let interleave = async {
            selected.await.unwrap();
            if fixed_first {
                assert!(!sibling.pools.ensure_fixed_defaults().await.unwrap());
            }
            resume.send(()).unwrap();
        };
        let (outcome, ()) = tokio::time::timeout(DEADLINE, async {
            tokio::join!(pointer.controller.bootstrap(), interleave)
        })
        .await
        .unwrap();
        outcome.unwrap();
        if !fixed_first {
            assert!(sibling.pools.ensure_fixed_defaults().await.unwrap());
            assert!(sibling.pools.default_held(false));
            assert!(!sibling.pools.default_held(true));
        }
        let new_target = sibling
            .target_for(&fixture.plan(Lane::Agent, 'b', capacity()))
            .await;
        assert_eq!(new_target.activated_at.is_some(), fixed_first);
        assert_eq!(pointer.pools.default_held(false), fixed_first);
        assert!(!pointer.pools.default_held(true));
        let release = pointer
            .controller
            .release
            .lanes
            .load(ReleasedLane::Agent)
            .await
            .unwrap();
        if fixed_first {
            assert!(release.is_none());
            assert!(pointer
                .admit(Lane::Agent, "held-after-activation")
                .await
                .is_err());
            assert!(pointer
                .controller
                .release
                .lanes
                .load(ReleasedLane::Machine)
                .await
                .unwrap()
                .is_none());
        } else {
            assert_eq!(release.unwrap().active.image, agent.spec.image.uri);
            assert_eq!(
                runtime(
                    &pointer
                        .admit(Lane::Agent, "after-publication")
                        .await
                        .unwrap()
                ),
                agent.spec.image.uri
            );
        }
        let after = legacy.sandbox(&retained.id).await.unwrap();
        assert_eq!(after.runtime, retained.runtime);
        assert_eq!(after.lifecycle.home, retained.lifecycle.home);
        fixture.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_pointer_publication_refuses_a_writer_lost_after_target_selection() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let target = original
        .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
        .await;
    let pointer = bind_controller(&original, |_| panic!("startup only reads MongoDB"), true).await;
    pointer.controller.register_boot().await.unwrap();
    let (selected, resume) = original.retirement.hold_selection(&target.id);
    let remove_writer = async {
        selected.await.unwrap();
        let field = format!("writers.{}", pointer.writer.incarnation);
        let removed = fixture
            .database
            .collection::<Document>(COLLECTION)
            .update_one(
                doc! {"_id": format!("{DEPLOYMENT}:writers")},
                doc! {"$unset": {field: ""}},
            )
            .await
            .unwrap();
        assert_eq!(removed.modified_count, 1);
        resume.send(()).unwrap();
    };
    let (outcome, ()) = tokio::time::timeout(DEADLINE, async {
        tokio::join!(pointer.controller.bootstrap(), remove_writer)
    })
    .await
    .unwrap();
    assert!(outcome.is_err());
    assert!(pointer
        .controller
        .release
        .lanes
        .load(ReleasedLane::Agent)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .database
        .collection::<Document>("native_warm_pool_targets")
        .find_one(doc! {"_id": target.id, "durable_release_authority": true})
        .await
        .unwrap()
        .is_none());
    fixture.database.drop().await.unwrap();
}
