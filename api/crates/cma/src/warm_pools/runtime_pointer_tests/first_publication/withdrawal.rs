use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn held_agent_lane_prevents_withdrawn_machine_selection() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    let fixed = fixture.replica_pinned_to('b').await;
    let agent = fixed
        .pools
        .prepare_released_image(&image(Lane::Agent, 'b'), false)
        .await
        .unwrap();
    fixed.store.activate(&agent.id).await.unwrap();
    let machine = original
        .pools
        .prepare_released_image(&image(Lane::Machine, '1'), true)
        .await
        .unwrap();
    assert!(machine.activated_at.is_none());
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    let (reached, resume) = original.retirement.hold_preparation(&machine.id);
    let activate_newer = async {
        reached.await.unwrap();
        assert!(publisher.pools.default_held(false));
        fixture
            .database
            .collection::<Document>("platform_settings")
            .update_one(
                doc! {"_id":format!("runtime_pointer_cutover:{DEPLOYMENT}"),"state":"fenced"},
                doc! {"$set":{"state":"withdrawn"}},
            )
            .await
            .unwrap();
        let next = fixed
            .pools
            .candidate_target(&image(Lane::Machine, '2'), &machine)
            .unwrap();
        let next = fixed.store.insert(next).await.unwrap();
        fixed.store.activate(&next.id).await.unwrap();
        fixed
            .store
            .verify_bootstrap(true, &next.plan.spec.image.uri)
            .await
            .unwrap();
        resume.send(()).unwrap();
        next
    };
    let (outcome, next) = tokio::time::timeout(DEADLINE, async {
        tokio::join!(publisher.controller.bootstrap(), activate_newer)
    })
    .await
    .unwrap();
    outcome.unwrap();
    assert!(publisher.pools.default_held(true));
    assert_first_publication_absent(&fixture.database).await;
    assert!(publisher
        .controller
        .release
        .lanes
        .load(ReleasedLane::Machine)
        .await
        .unwrap()
        .is_none());
    let unchanged = fixed.store.find(&machine.id).await.unwrap().unwrap();
    assert!(unchanged.activated_at.is_none() && unchanged.machine_plan_activated_at.is_none());
    fixed
        .store
        .verify_bootstrap(true, &next.plan.spec.image.uri)
        .await
        .unwrap();
    let pin = image(Lane::Agent, 'b');
    let machine_pin = image(Lane::Machine, '2');
    let released = Arc::new(
        StoreReleasedRuntime::open(
            &fixed.database,
            DEPLOYMENT,
            PinnedReleasedRuntime::new(pin.clone(), Some(machine_pin.clone())).snapshot(),
            true,
        )
        .await
        .unwrap()
        .unwrap(),
    );
    let fixed_store =
        crate::store::MongoSandboxStore::open(fixed.database.client(), fixed.database.name())
            .await
            .unwrap()
            .with_managed_runtime_version(pin.clone())
            .with_machine_runtime_version(Some(machine_pin.clone()));
    let admission =
        lifecycle::Lifecycle::with_store(&fixed, released, pin, machine_pin.clone(), fixed_store)
            .await
            .unwrap();
    assert_eq!(
        runtime(
            &admission
                .admit(Lane::Machine, "machine-after-withdrawal")
                .await
                .unwrap()
        ),
        machine_pin
    );
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn withdrawn_publisher_cannot_displace_a_compatible_fixed_pin() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    let prepared = original
        .pools
        .prepare_released_image(&image(Lane::Agent, 'a'), false)
        .await
        .unwrap();
    assert!(prepared.activated_at.is_none());
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    let (reached, resume) = original.retirement.hold_preparation(&prepared.id);
    let fixed = async {
        reached.await.unwrap();
        fixture
            .database
            .collection::<Document>("platform_settings")
            .update_one(
                doc! {"_id":format!("runtime_pointer_cutover:{DEPLOYMENT}"),"state":"fenced"},
                doc! {"$set":{"state":"withdrawn"}},
            )
            .await
            .unwrap();
        let newer = fixture.replica_pinned_to('b').await;
        assert!(!newer.pools.ensure_fixed_defaults().await.unwrap());
        newer
            .store
            .verify_bootstrap(false, &image(Lane::Agent, 'b'))
            .await
            .unwrap();
        resume.send(()).unwrap();
        newer
    };
    let (outcome, fixed) = tokio::time::timeout(DEADLINE, async {
        tokio::join!(publisher.controller.bootstrap(), fixed)
    })
    .await
    .unwrap();
    assert!(outcome.is_err());
    assert_first_publication_absent(&fixture.database).await;
    assert!(fixed
        .store
        .find(&prepared.id)
        .await
        .unwrap()
        .unwrap()
        .activated_at
        .is_none());
    fixed
        .store
        .verify_bootstrap(false, &image(Lane::Agent, 'b'))
        .await
        .unwrap();
    let plan = fixture.plan(Lane::Agent, 'b', capacity());
    fixture
        .provider
        .configure_planned(&plan, TARGET)
        .await
        .unwrap();
    fixture.readiness(&plan, TARGET);
    let pin = image(Lane::Agent, 'b');
    let machine_pin = image(Lane::Machine, '1');
    let released = Arc::new(
        StoreReleasedRuntime::open(
            &fixed.database,
            DEPLOYMENT,
            PinnedReleasedRuntime::new(pin.clone(), Some(machine_pin.clone())).snapshot(),
            true,
        )
        .await
        .unwrap()
        .unwrap(),
    );
    let fixed_store =
        crate::store::MongoSandboxStore::open(fixed.database.client(), fixed.database.name())
            .await
            .unwrap()
            .with_managed_runtime_version(pin.clone());
    let admission =
        lifecycle::Lifecycle::with_store(&fixed, released, pin.clone(), machine_pin, fixed_store)
            .await
            .unwrap();
    assert_eq!(
        runtime(
            &admission
                .admit(Lane::Agent, "fixed-after-withdrawal")
                .await
                .unwrap()
        ),
        pin
    );
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn withdrawn_machine_publisher_cannot_select_its_prepared_plan() {
    let fixture = Fixture::new().await;
    let original = fixture.replica().await;
    let agent = original
        .pools
        .prepare_released_image(&image(Lane::Agent, 'a'), false)
        .await
        .unwrap();
    let machine = original
        .pools
        .prepare_released_image(&image(Lane::Machine, '1'), true)
        .await
        .unwrap();
    let publisher = bind_controller(&original, |_| panic!("bootstrap reads MongoDB"), true).await;
    publisher.controller.register_boot().await.unwrap();
    let agent_record = ReleasedLaneRecord::bootstrap(
        ReleasedLane::Agent,
        agent.plan.spec.image.uri.clone(),
        &layer2_core::now_rfc3339(),
    )
    .unwrap();
    original
        .store
        .publish_bootstrap(
            &agent,
            &publisher.controller.release.lanes,
            &publisher.writer,
            &agent_record,
            uuid::Uuid::from_u128(1),
        )
        .await
        .unwrap();
    original
        .store
        .verify_cutover(uuid::Uuid::from_u128(1), &publisher.writer)
        .await
        .unwrap();
    fixture
        .database
        .collection::<Document>("platform_settings")
        .update_one(
            doc! {"_id":format!("runtime_pointer_cutover:{DEPLOYMENT}")},
            doc! {"$set":{"state":"withdrawn"}},
        )
        .await
        .unwrap();
    let newer = fixture.replica().await;
    let next = newer
        .pools
        .candidate_target(&image(Lane::Machine, '2'), &machine)
        .unwrap();
    let next = newer.store.insert(next).await.unwrap();
    newer.store.activate(&next.id).await.unwrap();
    let machine_record = ReleasedLaneRecord::bootstrap(
        ReleasedLane::Machine,
        machine.plan.spec.image.uri.clone(),
        &layer2_core::now_rfc3339(),
    )
    .unwrap();
    assert!(original
        .store
        .publish_bootstrap(
            &machine,
            &publisher.controller.release.lanes,
            &publisher.writer,
            &machine_record,
            uuid::Uuid::from_u128(1)
        )
        .await
        .is_err());
    let unchanged = newer.store.find(&machine.id).await.unwrap().unwrap();
    assert!(unchanged.activated_at.is_none() && unchanged.machine_plan_activated_at.is_none());
    newer
        .store
        .verify_bootstrap(true, &next.plan.spec.image.uri)
        .await
        .unwrap();
    assert_eq!(
        publisher
            .controller
            .release
            .lanes
            .load(ReleasedLane::Agent)
            .await
            .unwrap(),
        Some(agent_record)
    );
    assert!(publisher
        .controller
        .release
        .lanes
        .load(ReleasedLane::Machine)
        .await
        .unwrap()
        .is_none());
    fixture.database.drop().await.unwrap();
}
