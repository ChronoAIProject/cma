use super::*;
use crate::{startup::pointer_pool_startup, warm_pools::startup::WarmPoolStartup};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_pointer_startup_holds_a_superseded_pin_without_bootstrapping_it() {
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
