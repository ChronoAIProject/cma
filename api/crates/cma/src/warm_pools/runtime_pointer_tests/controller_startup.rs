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
