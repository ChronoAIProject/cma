use super::*;

async fn promote(rig: &Rig, leader: &dyn PointerReplica, lane: Lane, release: char) {
    let plan = rig.prepare(leader, lane, release).await;
    rig.fixture.readiness(&plan, TARGET);
    leader.reconcile(lane).await.unwrap();
    let released = leader.durable_release(lane).await;
    assert_eq!(released.active, image(lane, release));
    assert_eq!(released.candidate, None);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn replica_missing_refresh_never_allocates_the_old_digest() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let stale = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), stale.as_ref()).await;
    stale.refresh().await.unwrap();
    let a = rig.fixture.plan(Lane::Agent, 'a', capacity());
    let b = rig.fixture.plan(Lane::Agent, 'b', capacity());
    let before = rig.fixture.provider.inventory.lock().unwrap().effects.len();
    promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
    // Bypass the follower's periodic refresh. Admission must discover the
    // stale revision and retry against the durable release.
    let fresh = stale.admit(Lane::Agent, "refresh-missed").await.unwrap();
    assert_eq!(runtime(&fresh), image(Lane::Agent, 'b'));
    {
        let inventory = rig.fixture.provider.inventory.lock().unwrap();
        let claims = &inventory.effects[before..];
        assert!(!claims.contains(&Effect::Claim(a.config_sha256)));
        assert!(claims.contains(&Effect::Claim(b.config_sha256)));
    }
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn readiness_is_scoped_to_lane_binding_and_size() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    let agent = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    let machine = rig.prepare(leader.as_ref(), Lane::Machine, '2').await;
    let other_size = rig.fixture.plan(
        Lane::Agent,
        'b',
        SandboxCapacity {
            cpu_millis: 2000,
            ..capacity()
        },
    );
    rig.fixture
        .provider
        .configure_planned(&other_size, TARGET)
        .await
        .unwrap();
    rig.fixture.readiness(&other_size, TARGET);
    rig.fixture.readiness(&agent, TARGET - 1);
    rig.fixture.readiness(&machine, TARGET);
    leader.reconcile(Lane::Agent).await.unwrap();
    leader.reconcile(Lane::Machine).await.unwrap();
    assert_eq!(
        follower.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'a')
    );
    assert_eq!(
        follower.durable_release(Lane::Machine).await.active,
        image(Lane::Machine, '2')
    );
    assert_eq!(
        runtime(&follower.admit(Lane::Agent, "agent").await.unwrap()),
        image(Lane::Agent, 'a')
    );
    assert_eq!(
        runtime(&follower.admit(Lane::Machine, "machine").await.unwrap()),
        image(Lane::Machine, '2')
    );
    rig.fixture.readiness(&agent, TARGET);
    leader.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(
        follower.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'b')
    );
    assert_eq!(
        follower.promotion_history(Lane::Machine).await,
        [image(Lane::Machine, '2')]
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn zero_candidate_target_cannot_authorize_a_promotion() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    let candidate = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
    leader.resize_preparation(&candidate, 0).await;
    let _outcome = leader.reconcile(Lane::Agent).await;
    assert_eq!(rig.first.target_for(&candidate).await.target_replicas, 0);
    assert!(rig
        .first
        .target_for(&candidate)
        .await
        .activated_at
        .is_none());
    assert!(follower.promotion_history(Lane::Agent).await.is_empty());
    assert_eq!(
        runtime(&follower.admit(Lane::Agent, "zero-target").await.unwrap()),
        image(Lane::Agent, 'a')
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn registry_outage_preserves_admission_and_blocks_ready_candidate() {
    let mut rig = Rig::new().await;
    let registry = rig.registry.clone();
    let leader = wire(
        &rig.fixture,
        &rig.first,
        move |lane| registry.resolve(lane),
        true,
    )
    .await;
    let candidate = {
        let follower = rig.controller(&rig.second).await;
        rig.boot(leader.as_ref(), follower.as_ref()).await;
        rig.prepare(leader.as_ref(), Lane::Agent, 'b').await
    };
    rig.fixture.readiness(&candidate, TARGET);
    rig.registry.outage(true);
    let _outcome = leader.reconcile(Lane::Agent).await;
    assert_eq!(
        leader.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'a')
    );
    let registry_requests = rig.registry.request_count();
    rig.second = rig.fixture.restart(rig.second).await;
    let follower = rig.controller(&rig.second).await;
    follower.bootstrap().await.unwrap();
    assert_eq!(
        rig.registry.request_count(),
        registry_requests,
        "startup must use the durable release without consulting the registry"
    );
    assert_eq!(
        runtime(&follower.admit(Lane::Agent, "outage").await.unwrap()),
        image(Lane::Agent, 'a')
    );
    rig.registry.outage(false);
    leader.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(
        follower.promotion_history(Lane::Agent).await,
        [image(Lane::Agent, 'b')]
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn promotion_keeps_the_original_claim_until_its_adoption_finishes() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let registry = rig.registry.clone();
    let follower =
        controller::bind_controller(&rig.second, move |lane| registry.resolve(lane), true).await;
    rig.boot(leader.as_ref(), &follower).await;
    let lifecycle = follower.lifecycle().await.unwrap();
    let creating = lifecycle.create_agent_sandbox("in-flight").await.unwrap();
    let old = rig.fixture.plan(Lane::Agent, 'a', capacity());
    let (claimed, resume) = rig.second.provider.hold_claim();
    let change_release = async {
        let claim = at_gate(claimed).await;
        assert_eq!(claim.request.pool.config_sha256, old.config_sha256);
        promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
        leader.expire_release_settle(Lane::Agent).await;
        let _outcome = leader.retire_once(&old).await;
        assert_undrained(&rig.fixture, &old);
        resume.send(()).unwrap();
        claim
    };
    let (allocated, claim) = tokio::join!(lifecycle.drive_active(&creating.id), change_release);
    let allocated = allocated.unwrap();
    assert_eq!(runtime(&allocated), image(Lane::Agent, 'a'));
    let home = allocated.lifecycle.home.unwrap();
    assert_eq!(home.native_pool.unwrap().claim.as_ref(), Some(&claim));
    assert_eq!(home.request, claim.request.home);
    assert_eq!(
        runtime(&follower.admit(Lane::Agent, "after-flight").await.unwrap()),
        image(Lane::Agent, 'b')
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn promotion_preserves_the_first_claim_of_a_persisted_home_request() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let registry = rig.registry.clone();
    let follower =
        controller::bind_controller(&rig.second, move |lane| registry.resolve(lane), true).await;
    rig.boot(leader.as_ref(), &follower).await;
    let lifecycle = follower.lifecycle().await.unwrap();
    let creating = lifecycle
        .create_agent_sandbox("before-first-claim")
        .await
        .unwrap();
    let old = rig.fixture.plan(Lane::Agent, 'a', capacity());
    let (requested, resume) = rig.second.provider.hold_claim_request();
    let change_release = async {
        let request = at_gate(requested).await;
        let stored = follower.sandbox(&creating.id).await;
        let reservation = stored.lifecycle.home.unwrap().native_pool.unwrap();
        assert_eq!(reservation.request, request);
        assert!(reservation.claim.is_none());
        assert_eq!(request.pool.config_sha256, old.config_sha256);
        assert!(rig
            .second
            .provider
            .inventory_claims()
            .await
            .unwrap()
            .is_empty());
        promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
        leader.expire_release_settle(Lane::Agent).await;
        leader.retire_once(&old).await.unwrap();
        assert!(!rig.first.target_for(&old).await.retiring);
        assert_undrained(&rig.fixture, &old);
        resume.send(()).unwrap();
        request
    };
    let (allocated, request) = tokio::join!(lifecycle.drive_active(&creating.id), change_release);
    let allocated = allocated.unwrap();
    assert_eq!(runtime(&allocated), image(Lane::Agent, 'a'));
    let result = allocated.lifecycle.home.unwrap().native_pool.unwrap();
    assert_eq!(result.claim.unwrap().request, request);
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn superseded_spares_can_drain_while_the_retained_home_stays_bound() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    let old = rig.fixture.plan(Lane::Agent, 'a', capacity());
    let existing = follower.admit(Lane::Agent, "retained-home").await.unwrap();
    let retained = existing
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    let origin = retained.receipt.native_authority.as_ref().unwrap();
    assert_eq!(
        rig.first.target_for(&old).await.binding.as_ref(),
        Some(&origin.claim.request.pool)
    );
    promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
    leader.expire_release_settle(Lane::Agent).await;
    leader.retire_once(&old).await.unwrap();
    let after = follower.sandbox(&existing.id).await;
    assert_eq!(after.lifecycle.home, existing.lifecycle.home);
    assert_eq!(runtime(&after), image(Lane::Agent, 'a'));
    {
        let inventory = rig.fixture.provider.inventory.lock().unwrap();
        assert!(inventory
            .effects
            .contains(&Effect::Drain(old.config_sha256.clone(), vec![retained])));
        assert_eq!(inventory.status(&old).desired_replicas, 0);
    }
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn already_admitted_update_keeps_its_target_after_another_promotion() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    let existing = follower.admit(Lane::Agent, "update-home").await.unwrap();
    promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
    follower.refresh().await.unwrap();
    let requested = follower
        .request_update(&existing.id, &image(Lane::Agent, 'b'))
        .await
        .unwrap();
    assert_eq!(
        requested.admitted_runtime_update_target(),
        Some(image(Lane::Agent, 'b').as_str())
    );
    promote(&rig, leader.as_ref(), Lane::Agent, 'c').await;
    follower.refresh().await.unwrap();
    let updated = follower.run_pinned_update(&existing.id).await.unwrap();
    assert_eq!(runtime(&updated), image(Lane::Agent, 'b'));
    assert_eq!(updated.lifecycle.home, existing.lifecycle.home);
    assert_eq!(
        follower.durable_release(Lane::Agent).await.active,
        image(Lane::Agent, 'c')
    );
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn rollback_keeps_promoted_digest_for_every_new_allocation() {
    let rig = Rig::new().await;
    let leader = rig.controller(&rig.first).await;
    let follower = rig.controller(&rig.second).await;
    rig.boot(leader.as_ref(), follower.as_ref()).await;
    promote(&rig, leader.as_ref(), Lane::Agent, 'b').await;
    drop(follower);
    drop(leader);
    let rolled_back = rig.fixture.restart(rig.second).await;
    assert!(
        matches!(
            rolled_back.pools.ensure_default().await,
            Err(crate::warm_pools::WarmPoolError::Conflict)
        ),
        "a legacy binary pinned to A must fail the historical release-order guard"
    );
    let frozen = wire(
        &rig.fixture,
        &rolled_back,
        |_| panic!("pointer following is disabled"),
        false,
    )
    .await;
    frozen.bootstrap().await.unwrap();
    let after = frozen.admit(Lane::Agent, "rollback").await.unwrap();
    assert_eq!(runtime(&after), image(Lane::Agent, 'b'));
    assert_eq!(
        frozen.promotion_history(Lane::Agent).await,
        [image(Lane::Agent, 'b')]
    );
    rig.fixture.database.drop().await.unwrap();
}
