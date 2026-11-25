use super::*;
use crate::{
    store::native_warm_pools::{MongoNativeWarmPools, COLLECTION},
    warm_pools::{PoolTarget, PoolTargetStore as _},
};
use layer3_adapter::agent_sandbox::{NativePoolBinding, NativePoolStatus};
use mongodb::bson::{DateTime, Document};

fn target(id: &str, lane: ReleasedLane, digit: char) -> PoolTarget {
    let mut target = crate::warm_pools::tests::target();
    target.id = id.into();
    target.plan.deployment = "deployment-a".into();
    target.plan.spec.image.uri = image(digit);
    target.machine = lane == ReleasedLane::Machine;
    target.document_schema_version = if target.machine { 2 } else { 1 };
    target.plan.config_sha256 = digit.to_string().repeat(64);
    target.applied_revision = target.revision;
    target.binding = Some(NativePoolBinding {
        spec: target.plan.spec.clone(),
        pod_policy: target.plan.pod_policy.clone(),
        name: id.into(),
        uid: format!("{id}-pool-uid"),
        template_name: "pool-template".into(),
        template_uid: format!("{id}-template-uid"),
        config_sha256: target.plan.config_sha256.clone(),
    });
    target
}

async fn bootstrap(
    pools: &MongoNativeWarmPools,
    lanes: &MongoReleasedRuntimes,
    lane: ReleasedLane,
    id: &str,
    digit: char,
) -> (PoolTarget, ReleasedLaneRecord) {
    let mut target = target(id, lane, digit);
    target.activated_at = Some(DateTime::from_millis(1));
    pools.insert(target.clone()).await.unwrap();
    pools.retain_release_authority(&target).await.unwrap();
    let lane = ReleasedLaneRecord::bootstrap(lane, image(digit), "t0").unwrap();
    let saved = lanes.bootstrap(lane).await.unwrap();
    (target, saved)
}

async fn promote(
    pools: &MongoNativeWarmPools,
    lanes: &MongoReleasedRuntimes,
    current: &ReleasedLaneRecord,
    active: &PoolTarget,
    id: &str,
    digit: char,
    timestamp: i64,
) -> (PoolTarget, ReleasedLaneRecord) {
    let enabled = current.enable_following().unwrap();
    if current.frozen {
        lanes.replace(current, enabled.clone(), None).await.unwrap();
    }
    let observation = format!("obs-{digit}");
    let observed = enabled
        .observe(image(digit), observation.clone(), "t")
        .unwrap();
    let candidate = target(id, current.lane, digit);
    let mut session = lanes.session().await.unwrap();
    pools
        .prepare_candidate(
            candidate.clone(),
            observed.candidate.as_ref().unwrap(),
            &mut session,
        )
        .await
        .unwrap();
    lanes
        .replace(&enabled, observed.clone(), Some(&mut session))
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();

    let ready = observed
        .settle(&observation, CandidateState::Ready)
        .unwrap();
    let (next, receipt) = ready
        .promote(&evidence(&ready, digit, &observation), "t")
        .unwrap();
    let ready_pool = NativePoolStatus {
        binding: candidate.binding.clone().unwrap(),
        desired_replicas: candidate.target_replicas,
        ready_replicas: candidate.target_replicas,
        observed: true,
    };
    let mut session = lanes.session().await.unwrap();
    pools
        .promote_candidate(
            &observed,
            active,
            &candidate,
            &ready_pool,
            DateTime::from_millis(timestamp),
            &mut session,
        )
        .await
        .unwrap();
    lanes
        .record_promotion(&observed, next.clone(), receipt.clone(), &mut session)
        .await
        .unwrap();
    lanes
        .finish_promotion(&mut session, &receipt)
        .await
        .unwrap();
    (pools.find(id).await.unwrap().unwrap(), next)
}

async fn freeze(lanes: &MongoReleasedRuntimes, active: &ReleasedLaneRecord) -> ReleasedLaneRecord {
    let frozen = active.freeze().unwrap();
    lanes.replace(active, frozen.clone(), None).await.unwrap();
    frozen
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn operator_transition_preflight_requires_current_frozen_pair_and_retained_targets() {
    let (client, name, lanes) = lanes().await;
    let database = client.database(&name);
    let pools = MongoNativeWarmPools::open(&database, "deployment-a")
        .await
        .unwrap();
    let (agent_a, original) = bootstrap(&pools, &lanes, ReleasedLane::Agent, "agent-a", 'a').await;
    let (agent_b, first) = promote(&pools, &lanes, &original, &agent_a, "agent-b", 'b', 2).await;
    let (_, current) = promote(&pools, &lanes, &first, &agent_b, "agent-c", 'c', 3).await;
    let frozen = freeze(&lanes, &current).await;
    let (machine_d, original_machine) =
        bootstrap(&pools, &lanes, ReleasedLane::Machine, "machine-d", 'd').await;
    let (_, current_machine) = promote(
        &pools,
        &lanes,
        &original_machine,
        &machine_d,
        "machine-e",
        'e',
        2,
    )
    .await;
    let frozen_machine = freeze(&lanes, &current_machine).await;
    let proposal = |agent_revision, agent_target| {
        [
            TransitionDestination {
                lane: ReleasedLane::Agent,
                expected_revision: agent_revision,
                target_id: agent_target,
            },
            TransitionDestination {
                lane: ReleasedLane::Machine,
                expected_revision: frozen_machine.revision,
                target_id: "machine-e",
            },
        ]
    };
    let pool_rows = database.collection::<Document>(COLLECTION);
    let exact = doc! {"_id": "agent-b"};
    let before = pool_rows.find_one(exact.clone()).await.unwrap().unwrap();
    assert!(!before.contains_key("durable_release_authority"));

    for previous in ["agent-a", "agent-b"] {
        let [agent, machine] = proposal(frozen.revision, previous);
        let result =
            serde_json::to_value(lanes.inspect_transition_pair(agent, machine).await.unwrap())
                .unwrap();
        assert_eq!(result[0]["active_image"], image('c'));
        assert_eq!(result[0]["target_id"], previous);
        assert_eq!(result[1]["destination_image"], image('e'));
    }
    assert_eq!(
        pool_rows.find_one(exact.clone()).await.unwrap().unwrap(),
        before
    );

    for [agent, machine] in [
        proposal(current.revision, "agent-b"),
        proposal(frozen.revision, "machine-e"),
        proposal(frozen.revision, "agent-c"),
    ] {
        assert!(matches!(
            lanes.inspect_transition_pair(agent, machine).await,
            Err(SandboxStoreError::Conflict)
        ));
    }

    for unavailable in [
        doc! {"retiring": true, "target_replicas": 0},
        doc! {"target_replicas": 0},
        doc! {"applied_revision": 0_i64},
        doc! {"plan.spec.image.uri": image('f'), "binding.spec.image.uri": image('f')},
    ] {
        pool_rows
            .update_one(exact.clone(), doc! {"$set": unavailable})
            .await
            .unwrap();
        let invalid = pool_rows.find_one(exact.clone()).await.unwrap().unwrap();
        let [agent, machine] = proposal(frozen.revision, "agent-b");
        assert!(matches!(
            lanes.inspect_transition_pair(agent, machine).await,
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(
            pool_rows.find_one(exact.clone()).await.unwrap().unwrap(),
            invalid
        );
        pool_rows
            .replace_one(exact.clone(), before.clone())
            .await
            .unwrap();
    }

    // A different plan can share the image's original activation timestamp.
    // When both rows exist the historical receipt no longer identifies a pool.
    let mut wrong = target("agent-b-wrong-pool", ReleasedLane::Agent, 'b');
    wrong.plan.config_sha256 = "f".repeat(64);
    wrong.binding.as_mut().unwrap().config_sha256 = wrong.plan.config_sha256.clone();
    wrong.activated_at = agent_b.activated_at;
    pools.insert(wrong).await.unwrap();
    for target_id in ["agent-b", "agent-b-wrong-pool"] {
        let [agent, machine] = proposal(frozen.revision, target_id);
        assert!(matches!(
            lanes.inspect_transition_pair(agent, machine).await,
            Err(SandboxStoreError::Conflict)
        ));
    }
    assert_eq!(
        lanes.load(ReleasedLane::Agent).await.unwrap().unwrap(),
        frozen
    );
    assert_eq!(
        lanes.load(ReleasedLane::Machine).await.unwrap().unwrap(),
        frozen_machine
    );
    database.drop().await.unwrap();
}
