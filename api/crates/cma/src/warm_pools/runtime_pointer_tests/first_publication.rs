use super::*;

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
        let retained_replica = fixture.replica().await;
        let legacy_store = crate::store::MongoSandboxStore::open(
            retained_replica.database.client(),
            retained_replica.database.name(),
        )
        .await
        .unwrap()
        .with_managed_runtime_version(agent.spec.image.uri.clone());
        let legacy = lifecycle::Lifecycle::with_store(
            &retained_replica,
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
        .find_one(doc! {"_id": &target.id, "durable_release_authority": true})
        .await
        .unwrap()
        .is_none());
    drop(pointer);
    for release in ['b', 'c'] {
        let fixed = fixture.replica_pinned_to(release).await;
        let pin = image(Lane::Agent, release);
        let pins =
            PinnedReleasedRuntime::new(pin.clone(), Some(image(Lane::Machine, '1'))).snapshot();
        assert!(
            StoreReleasedRuntime::open(&fixed.database, DEPLOYMENT, pins, false)
                .await
                .unwrap()
                .is_none()
        );
        let next = fixed.pools.candidate_target(&pin, &target).unwrap();
        let staged = fixed.store.insert(next).await.unwrap();
        let written = legacy_activate(&fixed.database, &staged.id)
            .await
            .expect("abandoned publication must allow successive fixed-pin releases");
        assert_eq!(written.modified_count, 1);
        fixed.store.verify_bootstrap(false, &pin).await.unwrap();
    }
    fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn first_publication_orders_the_pre_fence_replica_activation_write() {
    for fixed_first in [true, false] {
        let fixture = Fixture::new().await;
        let publisher = fixture.replica().await;
        publisher.seed_defaults().await;
        let older = fixture.replica_pinned_to('b').await;
        let pin = image(Lane::Agent, 'b');
        let pins =
            PinnedReleasedRuntime::new(pin.clone(), Some(image(Lane::Machine, '1'))).snapshot();
        assert!(
            StoreReleasedRuntime::open(&older.database, DEPLOYMENT, pins, false)
                .await
                .unwrap()
                .is_none()
        );
        let original = publisher
            .target_for(&fixture.plan(Lane::Agent, 'a', capacity()))
            .await;
        let staged = older.pools.candidate_target(&pin, &original).unwrap();
        let staged = older.store.insert(staged).await.unwrap();
        // The fixed-pin replica already passed open() and inserted its unactivated row.
        let pointer =
            bind_controller(&publisher, |_| panic!("bootstrap reads MongoDB"), true).await;
        pointer.controller.register_boot().await.unwrap();
        let (selected, resume) = publisher.retirement.hold_selection(&original.id);
        let legacy_write = async {
            selected.await.unwrap();
            if fixed_first {
                assert_eq!(
                    legacy_activate(&older.database, &staged.id)
                        .await
                        .unwrap()
                        .modified_count,
                    1
                );
            }
            resume.send(()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(DEADLINE, async {
            tokio::join!(pointer.controller.bootstrap(), legacy_write)
        })
        .await
        .unwrap();
        result.unwrap();
        let lane = pointer
            .controller
            .release
            .lanes
            .load(ReleasedLane::Agent)
            .await
            .unwrap();
        if fixed_first {
            assert!(lane.is_none());
            assert!(pointer.pools.default_held(false));
        } else {
            assert_eq!(lane.unwrap().active.image, original.plan.spec.image.uri);
            let rejected = legacy_activate(&older.database, &staged.id)
                .await
                .unwrap_err();
            assert!(crate::store::is_duplicate_key(&rejected), "{rejected}");
        }
        assert_eq!(
            older
                .store
                .find(&staged.id)
                .await
                .unwrap()
                .unwrap()
                .activated_at
                .is_some(),
            fixed_first
        );
        fixture.database.drop().await.unwrap();
    }
}

async fn legacy_activate(
    database: &Database,
    id: &str,
) -> mongodb::error::Result<mongodb::results::UpdateResult> {
    // The original writer has found no activation timestamp for this image.
    database.collection::<Document>("native_warm_pool_targets").update_one(
        doc! {"_id":id,"plan.deployment":DEPLOYMENT,"document_schema_version":{"$in":[1_i64,2_i64]},
              "activated_at":mongodb::bson::Bson::Null,"retiring":{"$ne":true},"retired":{"$ne":true}},
        doc! {"$currentDate":{"activated_at":true}},
    ).await
}
