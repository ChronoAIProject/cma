use super::*;
use crate::store::{
    open_pointer_fixture_lifecycle_store, CreateResult, LifecycleBehavior, LifecycleInput,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn queued_managed_desktop_keeps_its_pool_until_handoff_after_promotion() {
    for (lane, previous, next) in [(Lane::Agent, 'b', 'c'), (Lane::Machine, '2', '3')] {
        let mut rig = Rig::new().await;
        let registry = rig.registry.clone();
        let (old, operation_id) = {
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
                resolved_image: Some(image(lane, previous)),
                capacity: Some(capacity()),
                sandbox_availability: Default::default(),
                capabilities: vec![],
                actor: None,
            };
            let CreateResult::Created(operation_id) = operations
                .create_against_release(
                    &lifecycle::owner().subject,
                    "released-desktop",
                    input,
                    Some((&release, lane.into())),
                )
                .await
                .unwrap()
            else {
                panic!("expected the first lifecycle admission");
            };
            promote(&rig, &first, lane, next).await;
            first.expire_release_settle(lane).await;
            first.retire_once(&old).await.unwrap();
            assert_undrained(&rig.fixture, &old);
            (old, operation_id)
        };

        // The new worker's bootstrap image and current lane both differ from
        // the digest captured before its Sandbox or home existed.
        rig.second = rig.fixture.restart(rig.second).await;
        let registry = rig.registry.clone();
        let restarted =
            bind_controller(&rig.second, move |lane| registry.resolve(lane), true).await;
        restarted.bootstrap().await.unwrap();
        let operations = open_pointer_fixture_lifecycle_store(
            &rig.second.database,
            &restarted.controller.release,
        )
        .await
        .unwrap();
        let operation = operations.claim(&operation_id).await.unwrap().unwrap();
        assert_eq!(operation.released_lane, Some(lane.into()));
        let lease = operation.lease().unwrap();
        let lifecycle = restarted.lifecycle().await.unwrap();
        let creating = lifecycle
            .create_pinned_lifecycle_sandbox(operation)
            .await
            .unwrap();
        assert!(creating.runtime.is_none() && creating.lifecycle.home.is_none());
        restarted.retire_once(&old).await.unwrap();
        assert_undrained(&rig.fixture, &old);
        let active = lifecycle.drive_active(&creating.id).await.unwrap();
        assert_eq!(runtime(&active), image(lane, previous));
        let retained = active
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        assert_eq!(
            retained
                .receipt
                .native_authority
                .as_ref()
                .unwrap()
                .claim
                .request
                .pool
                .config_sha256,
            old.config_sha256
        );
        operations.succeed(&lease, "agt_fixture").await.unwrap();
        restarted.retire_once(&old).await.unwrap();
        assert!(rig
            .fixture
            .provider
            .inventory
            .lock()
            .unwrap()
            .effects
            .contains(&Effect::Drain(old.config_sha256, vec![retained])));
        assert_eq!(
            lifecycle.sandbox(&active.id).await.unwrap().lifecycle.home,
            active.lifecycle.home
        );
        rig.fixture.database.drop().await.unwrap();
    }
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
