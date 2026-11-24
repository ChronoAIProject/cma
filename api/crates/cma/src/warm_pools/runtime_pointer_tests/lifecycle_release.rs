use super::*;
use crate::store::{
    open_pointer_fixture_lifecycle_store, CreateResult, LifecycleBehavior, LifecycleInput,
    MongoAgentLifecycleStore,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn queued_managed_desktop_keeps_its_pool_until_handoff_after_promotion() {
    for (lane, previous, next) in [(Lane::Agent, 'b', 'c'), (Lane::Machine, '2', '3')] {
        let mut rig = Rig::new().await;
        let (old, operation_id) = {
            let registry = rig.registry.clone();
            let first = bind_controller(&rig.first, move |lane| registry.resolve(lane), true).await;
            let second = rig.controller(&rig.second).await;
            rig.boot(&first, second.as_ref()).await;

            let old = promote(&rig, &first, lane, previous).await;
            let release = first.controller.release.snapshot();
            let operations = open_pointer_fixture_lifecycle_store(
                &rig.first.database,
                &first.controller.release,
            )
            .await
            .unwrap();
            let operation_id = queue_desktop(&operations, lane, previous, Some(&release)).await;
            promote(&rig, &first, lane, next).await;
            assert_queued_pool_is_protected(&rig, &first, lane, &old).await;
            (old, operation_id)
        };
        rig.second = rig.fixture.restart(rig.second).await;
        let registry = rig.registry.clone();
        let restarted =
            bind_controller(&rig.second, move |lane| registry.resolve(lane), true).await;
        restarted.bootstrap().await.unwrap();
        let lifecycle = restarted.lifecycle().await.unwrap();
        assert_cold_handoff(&rig, &restarted, lifecycle, &old, &operation_id, Some(lane)).await;
        rig.fixture.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unmarked_machine_admission_replays_a_historical_bootstrap_image() {
    let mut rig = Rig::new().await;
    let lane = Lane::Machine;
    let old = rig.fixture.plan(lane, '1', capacity());
    let legacy = MongoAgentLifecycleStore::open(&rig.first.database)
        .await
        .unwrap();
    let operation_id = queue_desktop(&legacy, lane, '1', None).await;
    {
        let registry = rig.registry.clone();
        let first = bind_controller(&rig.first, move |lane| registry.resolve(lane), true).await;
        let second = rig.controller(&rig.second).await;
        rig.boot(&first, second.as_ref()).await;
        crate::warm_pools::startup::await_ready(
            rig.first.store.as_ref(),
            &rig.first.provider,
            &old,
            DEADLINE,
        )
        .await
        .unwrap();
        promote(&rig, &first, lane, '2').await;
        assert_queued_pool_is_protected(&rig, &first, lane, &old).await;
    }

    rig.second = rig.fixture.restart(rig.second).await;
    let registry = rig.registry.clone();
    let restarted = bind_controller(&rig.second, move |lane| registry.resolve(lane), true).await;
    restarted.bootstrap().await.unwrap();
    let lifecycle = lifecycle::Lifecycle::new(
        &rig.second,
        restarted.controller.release.clone(),
        image(Lane::Agent, 'a'),
        image(lane, '2'),
    )
    .await
    .unwrap();
    assert_cold_handoff(&rig, &restarted, &lifecycle, &old, &operation_id, None).await;
    rig.fixture.database.drop().await.unwrap();
}

async fn queue_desktop(
    operations: &MongoAgentLifecycleStore,
    lane: Lane,
    digest: char,
    release: Option<&layer3_adapter::ReleasedRuntime>,
) -> String {
    let input = LifecycleInput {
        behavior: LifecycleBehavior::PromptRepository,
        managed_agent_instructions: None,
        setup_message: None,
        request: serde_json::from_value(serde_json::json!({
            "name": "Captured desktop",
            "desktop": lane == Lane::Agent,
            "machine_included": lane == Lane::Machine,
        }))
        .unwrap(),
        resolved_image: Some(image(lane, digest)),
        capacity: Some(capacity()),
        sandbox_availability: Default::default(),
        capabilities: vec![],
        actor: None,
    };
    let CreateResult::Created(operation_id) = operations
        .create_with_submission(
            &lifecycle::owner().subject,
            "released-desktop",
            input,
            None,
            release.map(|release| (release, lane.into())),
        )
        .await
        .unwrap()
    else {
        panic!("expected the first lifecycle admission");
    };
    operation_id
}

async fn assert_queued_pool_is_protected(
    rig: &Rig,
    first: &ControllerReplica<'_>,
    lane: Lane,
    old: &NativePoolConfiguration,
) {
    first.expire_release_settle(lane).await;
    first.retire_once(old).await.unwrap();
    assert_undrained(&rig.fixture, old);
}

async fn assert_cold_handoff(
    rig: &Rig,
    restarted: &ControllerReplica<'_>,
    lifecycle: &lifecycle::Lifecycle,
    old: &NativePoolConfiguration,
    operation_id: &str,
    recorded_lane: Option<Lane>,
) {
    let operations =
        open_pointer_fixture_lifecycle_store(&rig.second.database, &restarted.controller.release)
            .await
            .unwrap();
    let operation = operations.claim(operation_id).await.unwrap().unwrap();
    assert_eq!(
        operation.released_lane,
        recorded_lane.map(ReleasedLane::from)
    );
    let lease = operation.lease().unwrap();
    let creating = lifecycle
        .create_pinned_lifecycle_sandbox(operation)
        .await
        .unwrap();
    assert!(creating.runtime.is_none() && creating.lifecycle.home.is_none());
    restarted.retire_once(old).await.unwrap();
    assert_undrained(&rig.fixture, old);

    let active = lifecycle.drive_active(&creating.id).await.unwrap();
    assert_eq!(runtime(&active), old.spec.image.uri);
    let retained = active
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    assert_eq!(retained.request.policy, home_policy());
    let binding = rig.second.target_for(old).await.binding.unwrap();
    assert_eq!(
        retained
            .receipt
            .native_authority
            .as_ref()
            .unwrap()
            .claim
            .request
            .pool,
        binding
    );
    operations.succeed(&lease, "agt_fixture").await.unwrap();
    restarted.retire_once(old).await.unwrap();
    assert!(rig
        .fixture
        .provider
        .inventory
        .lock()
        .unwrap()
        .effects
        .contains(&Effect::Drain(old.config_sha256.clone(), vec![retained]),));
    assert_eq!(
        lifecycle.sandbox(&active.id).await.unwrap().lifecycle.home,
        active.lifecycle.home
    );
}

async fn promote(
    rig: &Rig,
    controller: &dyn PointerReplica,
    lane: Lane,
    release: char,
) -> NativePoolConfiguration {
    let plan = rig.prepare(controller, lane, release).await;
    rig.fixture.readiness(&plan, TARGET);
    controller.reconcile(lane).await.unwrap();
    assert_eq!(
        controller.durable_release(lane).await.active,
        image(lane, release)
    );
    plan
}
