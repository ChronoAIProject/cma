use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_promotion_refuses_old_spares_but_preserves_owned_replay() {
    let (client, name, store) = database().await;
    let old = plan();
    let store = store.with_managed_runtime_version(old.warm.runtime_version.clone());
    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let first = publish_ready(&store).await;
    let existing = admission(&store, &old, "owner", "before-promotion").await;
    let SandboxAdmissionResult::Created(owned) =
        admit_at(&store, existing.clone(), NOW).await.unwrap()
    else {
        panic!("fresh owned admission");
    };
    assert_eq!(owned.id, first.payload.id);

    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let spare = publish_ready(&store).await;
    let mut next = old.clone();
    next.warm.runtime_version = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    next.validate().unwrap();
    let promoted = store
        .clone()
        .with_managed_runtime_version(next.warm.runtime_version.clone());
    // Model the interval before the new preparation worker's first target sweep.
    let request = admission(&promoted, &next, "owner", "during-promotion").await;
    let mut session = promoted.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let outcome = super::super::admission::claim_ready(
        &promoted,
        &mut request.sandbox.clone(),
        NOW,
        &mut session,
    )
    .await
    .map_err(|_| ())
    .unwrap();
    session.abort_transaction().await.unwrap();
    assert_eq!(
        outcome,
        ClaimOutcome::Miss("miss_released_runtime_mismatch")
    );
    let SandboxAdmissionResult::Created(fallback) =
        admit_at(&promoted, request, NOW).await.unwrap()
    else {
        panic!("fresh admission must continue through ordinary provisioning");
    };
    assert!(fallback.runtime.is_none());
    assert_ne!(fallback.id, spare.payload.id);
    let retained = promoted
        .native_prepared_record(&spare.payload.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, spare);

    let SandboxAdmissionResult::Existing(replay) =
        admit_at(&promoted, existing, NOW).await.unwrap()
    else {
        panic!("previous ownership must remain replayable");
    };
    assert_eq!(replay, owned);
    assert_eq!(
        replay.runtime.unwrap().runtime_version,
        old.warm.runtime_version
    );

    promoted
        .reconcile_native_prepared_target(Some(&next), 1, NOW)
        .await
        .unwrap();
    let replacement = publish_ready(&promoted).await;
    let current = admission(&promoted, &next, "owner", "after-promotion").await;
    let SandboxAdmissionResult::Created(current) = admit_at(&promoted, current, NOW).await.unwrap()
    else {
        panic!("current prepared capacity must still be used");
    };
    assert_eq!(current.id, replacement.payload.id);
    assert_eq!(
        current.runtime.unwrap().runtime_version,
        next.warm.runtime_version
    );
    assert_eq!(
        promoted
            .native_prepared_record(&spare.payload.id)
            .await
            .unwrap()
            .unwrap()
            .payload
            .state,
        WarmSandboxState::Retiring
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn insert_only_refill_rechecks_the_released_lane_before_inserting() {
    use crate::store::released_runtimes::MongoReleasedRuntimes;
    use layer2_core::released_runtime::{
        CandidateState, PromotionEvidence, ReleasedLane, ReleasedLaneRecord,
    };
    use layer3_adapter::{ReleasedLaneRevisions, ReleasedRuntime};

    for machine in [false, true] {
        let (client, name, mut stale_replica) = database().await;
        let lane = if machine {
            ReleasedLane::Machine
        } else {
            ReleasedLane::Agent
        };
        let previous = if machine {
            super::machine_tests::machine_plan()
        } else {
            plan()
        };
        let image = format!("registry.example/replacement@sha256:{}", "d".repeat(64));
        let next = previous.for_runtime(&image).unwrap();
        let database = client.database(&name);
        let lanes = MongoReleasedRuntimes::open(&database, "prepared-fixture");
        lanes.initialize().await.unwrap();
        let first = ReleasedLaneRecord::bootstrap(lane, previous.warm.runtime_version.clone(), NOW)
            .unwrap();
        lanes.bootstrap(first.clone()).await.unwrap();
        let released = |version: &str, revision| ReleasedRuntime {
            revision,
            agent: if machine {
                "agent-fixture".into()
            } else {
                version.into()
            },
            machine: machine.then(|| version.to_owned()),
            lane_revisions: Some(ReleasedLaneRevisions {
                agent: if machine { 1 } else { revision },
                machine: machine.then_some(revision),
            }),
        };
        stale_replica
            .reconcile_native_prepared_class(Some(&previous), 1, NOW, machine)
            .await
            .unwrap();
        if machine {
            stale_replica = stale_replica
                .with_machine_runtime_version(Some(previous.warm.runtime_version.clone()));
            super::machine_tests::ready_machine(&stale_replica).await;
        } else {
            stale_replica =
                stale_replica.with_managed_runtime_version(previous.warm.runtime_version.clone());
            publish_ready(&stale_replica).await;
        }
        let request = admission(&stale_replica, &previous, "owner", "before-promotion").await;
        let SandboxAdmissionResult::Created(owned) =
            admit_at(&stale_replica, request, NOW).await.unwrap()
        else {
            panic!("expected prepared admission")
        };
        assert!(owned.runtime.is_some());
        stale_replica.released_lanes = Some(lanes.clone());

        let ready = first
            .enable_following()
            .unwrap()
            .observe(image.clone(), "obs".into(), NOW)
            .unwrap()
            .settle("obs", CandidateState::Ready)
            .unwrap();
        lanes.replace(&first, ready.clone(), None).await.unwrap();
        let (promoted, transition) = ready
            .promote(
                &PromotionEvidence {
                    image: image.clone(),
                    observation_id: "obs".into(),
                    lane_revision: ready.revision,
                },
                NOW,
            )
            .unwrap();
        let mut session = lanes.session().await.unwrap();
        lanes
            .record_promotion(&ready, promoted, transition, &mut session)
            .await
            .unwrap();
        session.commit_transaction().await.unwrap();

        assert!(matches!(
            stale_replica
                .reconcile_native_prepared_release(
                    Some(&previous),
                    1,
                    NOW,
                    machine,
                    &released(&previous.warm.runtime_version, 1),
                )
                .await,
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(records(&stale_replica).await.len(), 1);
        stale_replica
            .reconcile_native_prepared_release(Some(&next), 1, NOW, machine, &released(&image, 2))
            .await
            .unwrap();
        let records = records(&stale_replica).await;
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(
            |record| record.plan == next && record.payload.state == WarmSandboxState::Preparing
        ));
        database.drop().await.unwrap();
    }
}
