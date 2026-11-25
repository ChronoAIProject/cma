use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn pointer_replacement_beats_ready_inventory_already_in_flight() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let reader = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), reader.as_ref()).await;
    let ready = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    rig.fixture.readiness(&ready, TARGET);
    let (observed, resume) = rig.first.provider.hold_inventory();
    let replace = async {
        let observed = at_gate(observed).await;
        assert!(observed
            .iter()
            .any(|pool| pool.binding.config_sha256 == ready.config_sha256
                && pool.ready_replicas == TARGET));
        rig.registry.point(Lane::Agent, 'c');
        resume.send(()).unwrap();
    };
    let (_attempt, ()) = tokio::join!(leader.reconcile(Lane::Agent), replace);
    assert_eq!(
        reader.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'a')
    );
    assert!(reader.promotion_history(Lane::Agent).await.is_empty());
    leader.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(
        reader.durable_release(Lane::Agent).await.candidate,
        Some(image(Lane::Agent, 'c'))
    );
    assert!(rig.first.target_for(&ready).await.activated_at.is_none());
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn resolver_takeover_fences_a_ready_observation_from_the_old_epoch() {
    let rig = Rig::new().await;
    let old = rig.controller(&rig.first).await;
    let new = rig.controller(&rig.second).await;
    rig.boot(old.as_ref(), new.as_ref()).await;
    let candidate = rig.prepare(old.as_ref(), Lane::Agent, 'b').await;
    rig.fixture.readiness(&candidate, TARGET);
    let (observed, resume) = rig.first.provider.hold_inventory();
    let elect = async {
        at_gate(observed).await;
        new.expire_resolver_lease(Lane::Agent).await;
        new.reconcile(Lane::Agent).await.unwrap();
        let promoted = new.durable_release(Lane::Agent).await;
        assert_eq!(promoted.active, image(Lane::Agent, 'b'));
        resume.send(()).unwrap();
        promoted
    };
    let (_stale_attempt, promoted) = tokio::join!(old.reconcile(Lane::Agent), elect);
    assert_eq!(old.durable_release(Lane::Agent).await, promoted);
    assert_eq!(
        new.promotion_history(Lane::Agent).await,
        [image(Lane::Agent, 'b')]
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn promotion_revalidates_resize_after_old_target_has_reported_ready() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let changer = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), changer.as_ref()).await;
    let plan = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    rig.fixture.readiness(&plan, TARGET);
    let active = rig.fixture.plan(Lane::Agent, 'a', capacity());
    let before = rig.second.target_for(&active).await;
    let (observed, resume) = rig.first.provider.hold_inventory();
    let resize = async {
        at_gate(observed).await;
        changer.resize_preparation(&plan, TARGET + 1).await;
        let resized = rig.second.target_for(&active).await;
        assert_eq!(resized.target_replicas, TARGET + 1);
        assert!(resized.revision > before.revision);
        resume.send(()).unwrap();
    };
    let (_attempt, ()) = tokio::join!(leader.reconcile(Lane::Agent), resize);
    assert_eq!(
        changer.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'a')
    );
    assert!(changer.promotion_history(Lane::Agent).await.is_empty());
    leader.reconcile(Lane::Agent).await.unwrap();
    let pending = rig.second.target_for(&plan).await;
    assert_eq!(pending.target_replicas, TARGET + 1);
    assert!(pending.activated_at.is_none());
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_retirement_cannot_mark_or_drain_a_newly_protected_candidate() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let stale = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), stale.as_ref()).await;
    let plan = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    rig.registry.point(Lane::Agent, 'c');
    leader.reconcile(Lane::Agent).await.unwrap();
    stale.expire_release_settle(Lane::Agent).await;
    let (at_marker, resume) = rig.second.retirement.hold_marker(&plan);
    let protect = async {
        at_gate(at_marker).await;
        let target = rig.first.target_for(&plan).await;
        let write = rig
            .fixture
            .database
            .collection::<mongodb::bson::Document>("native_warm_pool_targets")
            .update_one(
                doc! {"_id": &target.id},
                doc! {"$set": {"lease_until": mongodb::bson::DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        assert_eq!(write.matched_count, 1);
        assert_eq!(rig.prepare(leader.as_ref(), Lane::Agent, 'b').await, plan);
        resume.send(()).unwrap();
    };
    let (_retirement, ()) = tokio::join!(stale.retire_once(&plan), protect);
    assert!(!rig.second.target_for(&plan).await.retiring);
    assert_undrained(&rig.fixture, &plan);
    leader
        .apply_target(&rig.first.target_for(&plan).await.id)
        .await;
    rig.fixture.readiness(&plan, TARGET);
    leader.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(
        stale.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'b')
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unknown_promotion_commit_recovers_once_from_durable_authority() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    let before = follower.durable_release(Lane::Agent).await;
    let candidate = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    rig.fixture.readiness(&candidate, TARGET);
    inject_unknown_commit(&rig.fixture, &rig.first, leader.reconcile(Lane::Agent)).await;
    let released = follower.durable_release(Lane::Agent).await;
    let first_activation = rig.second.target_for(&candidate).await.activated_at;
    assert!(first_activation.is_some());
    assert_eq!(released.active, image(Lane::Agent, 'b'));
    assert_eq!(released.active_revision, before.active_revision + 1);
    assert_eq!(released.candidate, None);
    follower.expire_resolver_lease(Lane::Agent).await;
    follower.reconcile(Lane::Agent).await.unwrap();
    leader.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(leader.durable_release(Lane::Agent).await, released);
    assert_eq!(
        leader.promotion_history(Lane::Agent).await,
        [image(Lane::Agent, 'b')]
    );
    assert_eq!(
        rig.first.target_for(&candidate).await.activated_at,
        first_activation
    );
    rig.fixture.database.drop().await.unwrap();
}
