use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_gate_holds_before_the_durable_marker() {
    let fixture = Fixture::new().await;
    let first = fixture.replica().await;
    let second = fixture.replica().await;
    let plan = fixture.plan(Lane::Agent, 'b', capacity());
    let original = first
        .store
        .insert(PoolTarget {
            plan: plan.clone(),
            ..target()
        })
        .await
        .unwrap();
    let (marker, resume) = first.retirement.hold_marker(&plan);
    let inspect = async {
        tokio::time::timeout(DEADLINE, marker)
            .await
            .expect("retirement must reach the marker")
            .unwrap();
        let pending = second.target_for(&plan).await;
        assert!(!pending.retiring);
        assert_eq!(pending.revision, original.revision);
        assert_eq!(pending.target_replicas, original.target_replicas);
        resume.send(()).unwrap();
    };
    let (updated, ()) = tokio::join!(first.retirement.begin_retirement(&original), inspect);
    let updated = updated.unwrap();
    let after = second.target_for(&plan).await;
    assert!(after.retiring);
    assert_eq!(after.revision, original.revision + 1);
    assert_eq!(after.revision, updated.revision);
    assert_eq!(after.target_replicas, 0);
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn restarted_replica_observes_the_original_ready_binding() {
    let fixture = Fixture::new().await;
    let first = fixture.replica().await;
    let second = fixture.replica().await;
    first.seed_defaults().await;
    let active = fixture.plan(Lane::Agent, 'a', capacity());
    let machine = fixture.plan(Lane::Machine, '1', capacity());
    let other_size = fixture.plan(
        Lane::Agent,
        'a',
        SandboxCapacity {
            cpu_millis: 2000,
            ..capacity()
        },
    );
    for plan in [&active, &machine, &other_size] {
        fixture
            .provider
            .configure_planned(plan, TARGET)
            .await
            .unwrap();
    }
    // Neither the same image at a different size nor the machine lane should
    // supply readiness for the default Agent plan.
    fixture.readiness(&other_size, TARGET);
    fixture.readiness(&active, 1);
    first.boot().await;
    let original = first.target_for(&active).await;
    let binding = original.binding.as_ref().unwrap();
    let restarted = fixture.restart(second).await;
    let (captured, resume) = restarted.provider.hold_inventory();
    let inspect = async {
        let inventory = tokio::time::timeout(DEADLINE, captured)
            .await
            .expect("restarted replica should fetch fresh inventory")
            .unwrap();
        let status = inventory
            .iter()
            .find(|status| &status.binding == binding)
            .unwrap();
        assert_eq!(status.ready_replicas, 1);
        assert!(status.observed);
        resume.send(()).unwrap();
    };
    tokio::join!(restarted.boot(), inspect);
    let after = restarted.target_for(&active).await;
    assert_eq!(after.binding, original.binding);
    assert_eq!(after.revision, original.revision);
    assert_eq!(after.applied_revision, after.revision);
    {
        let inventory = fixture.provider.inventory.lock().unwrap();
        assert_eq!(inventory.status(&machine).ready_replicas, 0);
        assert_eq!(inventory.status(&other_size).ready_replicas, TARGET);
        assert!(inventory.effects.iter().all(|effect| matches!(
            effect,
            Effect::Configure(_, TARGET) | Effect::Resize(_, TARGET)
        )));
    }
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn legacy_startup_refuses_the_superseded_pin_after_rollback() {
    use crate::warm_pools::WarmPoolError;
    use mongodb::bson::{doc, DateTime, Document};

    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    original.seed_defaults().await;
    let old = fixture.plan(Lane::Agent, 'a', capacity());
    let old_target = original.target_for(&old).await;
    // Separate releases can land inside one millisecond in the fixture. Give
    // the first release an earlier timestamp without a wall-clock sleep.
    let first_activation = DateTime::from_millis(1);
    let changed = fixture
        .database
        .collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id": &old_target.id},
            doc! {"$set": {"activated_at": first_activation}},
        )
        .await
        .unwrap();
    assert_eq!(changed.matched_count, 1);
    let released = fixture.replica_pinned_to('b').await;
    let new = fixture.plan(Lane::Agent, 'b', capacity());
    fixture
        .provider
        .configure_planned(&new, TARGET)
        .await
        .unwrap();
    fixture.readiness(&new, TARGET);
    released.boot().await;
    let new_target = released.target_for(&new).await;
    assert!(new_target.activated_at.unwrap() > first_activation);

    let stale = fixture.restart(original).await;
    assert!(matches!(
        stale.pools.ensure_default().await,
        Err(WarmPoolError::Conflict)
    ));
    assert_eq!(stale.target_for(&new).await.binding, new_target.binding);
    assert_eq!(
        stale.target_for(&old).await.activated_at,
        Some(first_activation)
    );
    let rolled_back = fixture.replica_pinned_to('b').await;
    rolled_back.boot().await;
    let current = rolled_back.target_for(&new).await;
    assert_eq!(current.activated_at, new_target.activated_at);
    assert_eq!(current.binding, new_target.binding);
    fixture.database.drop().await.unwrap();
}
