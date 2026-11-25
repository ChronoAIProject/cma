use super::*;
use layer2_core::released_runtime::{
    CandidateState, PromotionEvidence, ReleasedLane, ReleasedLaneRecord,
};

fn image(digit: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        digit.to_string().repeat(64)
    )
}

async fn lanes() -> (mongodb::Client, String, MongoReleasedRuntimes) {
    let (client, name, _) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let lanes = MongoReleasedRuntimes::open(&client.database(&name), "deployment-a");
    (client, name, lanes)
}

fn ready(record: &ReleasedLaneRecord, digit: char, observation: &str) -> ReleasedLaneRecord {
    let observed = record
        .observe(image(digit), observation.into(), "t")
        .unwrap();
    observed.settle(observation, CandidateState::Ready).unwrap()
}

fn evidence(record: &ReleasedLaneRecord, digit: char, observation: &str) -> PromotionEvidence {
    PromotionEvidence {
        image: image(digit),
        observation_id: observation.into(),
        lane_revision: record.revision,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bootstrap_is_once_and_writes_are_compare_and_swap() {
    let (client, name, lanes) = lanes().await;
    let first = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('a'), "t0").unwrap();
    let second = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('b'), "t1").unwrap();
    assert_eq!(lanes.bootstrap(first.clone()).await.unwrap(), first);
    // A later bootstrap from another pin never replaces the recorded release.
    assert_eq!(lanes.bootstrap(second).await.unwrap(), first);
    let enabled = first.enable_following().unwrap();
    lanes.replace(&first, enabled.clone(), None).await.unwrap();
    // A writer still holding the superseded record is refused.
    let stale = first.enable_following().unwrap();
    assert!(matches!(
        lanes.replace(&first, stale, None).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        lanes.load(ReleasedLane::Agent).await.unwrap().unwrap(),
        enabled
    );
    // Lanes are independent.
    assert!(lanes.load(ReleasedLane::Machine).await.unwrap().is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admission_fence_commits_only_while_its_release_is_active() {
    let (client, name, lanes) = lanes().await;
    let mut record = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('a'), "t0").unwrap();
    lanes.bootstrap(record.clone()).await.unwrap();
    let enabled = record.enable_following().unwrap();
    lanes.replace(&record, enabled.clone(), None).await.unwrap();
    record = enabled;
    let mut session = client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    lanes
        .fence_admission(ReleasedLane::Agent, &image('a'), 1, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();

    let prepared = ready(&record, 'b', "obs-b");
    lanes
        .replace(&record, prepared.clone(), None)
        .await
        .unwrap();
    let (promoted, transition) = prepared
        .promote(&evidence(&prepared, 'b', "obs-b"), "t2")
        .unwrap();
    commit_promotion(&client, &lanes, &prepared, promoted, transition.clone()).await;
    // An admission captured against A before the promotion now conflicts.
    let mut session = client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    assert!(matches!(
        lanes
            .fence_admission(ReleasedLane::Agent, &image('a'), 1, &mut session)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    session.abort_transaction().await.unwrap();
    let loaded = lanes.load(ReleasedLane::Agent).await.unwrap().unwrap();
    assert_eq!(loaded.active.image, image('b'));
    assert_eq!(loaded.admissions, 1);
    assert!(lanes.committed(&transition).await.unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn committed_transition_survives_later_promotions() {
    let (client, name, lanes) = lanes().await;
    let mut record = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('0'), "t0").unwrap();
    lanes.bootstrap(record.clone()).await.unwrap();
    let enabled = record.enable_following().unwrap();
    lanes.replace(&record, enabled.clone(), None).await.unwrap();
    record = enabled;
    let mut transitions = Vec::new();
    for (index, digit) in "123456789abcdef0123".chars().enumerate() {
        let observation = format!("obs-{index}");
        let prepared = ready(&record, digit, &observation);
        lanes
            .replace(&record, prepared.clone(), None)
            .await
            .unwrap();
        let (promoted, transition) = prepared
            .promote(&evidence(&prepared, digit, &observation), "t")
            .unwrap();
        commit_promotion(
            &client,
            &lanes,
            &prepared,
            promoted.clone(),
            transition.clone(),
        )
        .await;
        transitions.push(transition);
        record = promoted;
    }
    for transition in transitions {
        assert!(lanes.committed(&transition).await.unwrap());
    }
    client.database(&name).drop().await.unwrap();
}

async fn commit_promotion(
    client: &mongodb::Client,
    lanes: &MongoReleasedRuntimes,
    current: &ReleasedLaneRecord,
    next: ReleasedLaneRecord,
    transition: ReleasedTransition,
) {
    let mut session = client.start_session().await.unwrap();
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    lanes
        .record_promotion(current, next, transition, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn expired_boot_cannot_authorize_another_provider_effect() {
    let (client, name, lanes) = lanes().await;
    let writer = BootWriter {
        incarnation: uuid::Uuid::new_v4(),
        pod_uid: uuid::Uuid::new_v4(),
        build_digest: format!("sha256:{}", "a".repeat(64)),
    };
    lanes.publish_boot(&writer).await.unwrap();
    lanes.protect_boot_effect(&writer).await.unwrap();
    let database = client.database(&name);
    let roster = database.collection::<mongodb::bson::Document>(COLLECTION);
    let mut fields = doc! {};
    fields.insert(
        format!("writers.{}.expires_at", writer.incarnation),
        mongodb::bson::DateTime::from_millis(0),
    );
    roster
        .update_one(doc! {"_id":"deployment-a:writers"}, doc! {"$set": fields})
        .await
        .unwrap();
    assert!(matches!(
        lanes.protect_boot_effect(&writer).await,
        Err(SandboxStoreError::Conflict)
    ));
    lanes.remove_writer(&writer).await.unwrap();
    let held = roster
        .find_one(doc! {"_id":"deployment-a:writers"})
        .await
        .unwrap()
        .unwrap();
    assert!(held
        .get_document("writers")
        .unwrap()
        .contains_key(writer.incarnation.to_string()));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn release_authority_floor_refuses_fallback_even_when_lane_is_unreadable() {
    let (client, name, _) = lanes().await;
    let database = client.database(&name);
    let pin = layer3_adapter::PinnedReleasedRuntime::new(image('a'), None);
    use layer3_adapter::ReleasedRuntimeSource;
    assert!(
        StoreReleasedRuntime::open(&database, "deployment-a", pin.snapshot(), false)
            .await
            .unwrap()
            .is_none()
    );
    database
        .collection::<mongodb::bson::Document>(super::super::native_warm_pools::COLLECTION)
        .insert_one(
            doc! {"plan": {"deployment": "deployment-a"}, "durable_release_authority": true},
        )
        .await
        .unwrap();
    assert!(
        StoreReleasedRuntime::open(&database, "deployment-a", pin.snapshot(), false)
            .await
            .is_err()
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn compatible_first_claim_preserves_its_legacy_dispatch_pin_after_worker_loss() {
    use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
    use layer2_core::sandbox::{SandboxStatus, SandboxWorkState, SandboxWorkStep};

    let (client, name, store) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let database = client.database(&name);
    let mut compatible = store.with_managed_runtime_version(image('a'));
    compatible.released_lanes = Some(MongoReleasedRuntimes::open(&database, "deployment-a"));
    let now = layer2_core::now_rfc3339();
    let until =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60));
    let successor_until =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(120));
    let mut sandbox = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_legacy_dispatch",
    );
    sandbox.status = SandboxStatus::Provisioning;
    sandbox.runtime = None;
    sandbox.lifecycle = Default::default();
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, now.clone());
    compatible.create_sandbox(sandbox.clone()).await.unwrap();
    compatible
        .claim_sandbox_work(&sandbox.id, "first", now, until)
        .await
        .unwrap()
        .unwrap();

    expire_work_lease(&database, &sandbox.id).await;
    let successor = compatible.with_managed_runtime_version(image('b'));
    let resumed = successor
        .claim_sandbox_work(
            &sandbox.id,
            "successor",
            layer2_core::now_rfc3339(),
            successor_until,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        successor
            .reserve_initial_runtime_image(&resumed, 1, &image('b'))
            .await
            .unwrap(),
        image('a')
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn lost_legacy_warm_runtime_binds_the_reserved_replacement_generation() {
    use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
    use layer2_core::sandbox::{SandboxStatus, SandboxWorkState, SandboxWorkStep};

    let (client, name, mut writer) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let database = client.database(&name);
    writer.released_lanes = Some(MongoReleasedRuntimes::open(&database, "deployment-a"));
    let writer = writer.with_managed_runtime_version(image('a'));
    let at = |seconds| {
        layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(seconds),
        )
    };
    let mut warm = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_lost_legacy_warm",
    );
    warm.status = SandboxStatus::Provisioning;
    warm.runtime.as_mut().unwrap().runtime_version = image('a');
    warm.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, at(0));
    writer.create_sandbox(warm.clone()).await.unwrap();
    let mut claimed = writer
        .claim_sandbox_work(&warm.id, "first", at(0), at(60))
        .await
        .unwrap()
        .unwrap();

    let expected = claimed.revision;
    claimed.runtime = None;
    claimed.lifecycle.runtime_generation_high_water = 2;
    claimed.revision += 1;
    writer.save_sandbox(expected, claimed).await.unwrap();
    expire_work_lease(&database, &warm.id).await;
    let restarted = writer.with_managed_runtime_version(image('b'));
    let successor = restarted
        .claim_sandbox_work(&warm.id, "successor", at(0), at(120))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restarted
            .reserve_initial_runtime_image(&successor, 2, &image('b'))
            .await
            .unwrap(),
        image('a')
    );
    assert!(matches!(
        restarted
            .reserve_initial_runtime_image(&successor, 1, &image('b'))
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    database.drop().await.unwrap();
}

async fn expire_work_lease(database: &Database, sandbox: &layer2_core::SandboxId) {
    let past = "2026-09-05T00:00:00Z";
    let result = database
        .collection::<mongodb::bson::Document>("sandboxes")
        .update_one(
            doc! {"_id": sandbox.as_str()},
            doc! {"$set": {
                "work_lease_expires_at": past, "record.work.lease.expires_at": past,
            }},
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

#[path = "released_runtimes_tests/transition_preflight.rs"]
mod transition_preflight;
