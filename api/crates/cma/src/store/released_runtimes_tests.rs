use super::*;
use layer2_core::released_runtime::{
    CandidateState, FleetAttestation, PromotionEvidence, ReleasedLane, ReleasedLaneRecord,
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
        template_hash: "hash-1".into(),
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
    let attested = first
        .attest(FleetAttestation {
            generation: 1,
            template_hash: "hash-1".into(),
            attested_at: "t".into(),
        })
        .unwrap();
    lanes
        .replace(&first, attested.clone(), None, None)
        .await
        .unwrap();
    // A writer still holding the superseded record is refused.
    let stale = first.observe(image('c'), "obs-c".into(), "t").unwrap();
    assert!(matches!(
        lanes.replace(&first, stale, None, None).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        lanes.load(ReleasedLane::Agent).await.unwrap().unwrap().0,
        attested
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
    let attested = record
        .attest(FleetAttestation {
            generation: 1,
            template_hash: "hash-1".into(),
            attested_at: "t".into(),
        })
        .unwrap();
    lanes
        .replace(&record, attested.clone(), None, None)
        .await
        .unwrap();
    record = attested;
    let mut session = client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    lanes
        .fence_admission(ReleasedLane::Agent, &image('a'), 1, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();

    let prepared = ready(&record, 'b', "obs-b");
    lanes
        .replace(&record, prepared.clone(), None, None)
        .await
        .unwrap();
    let (promoted, transition) = prepared
        .promote(&evidence(&prepared, 'b', "obs-b"), "t2")
        .unwrap();
    lanes
        .replace(&prepared, promoted.clone(), Some(transition.clone()), None)
        .await
        .unwrap();
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
    let (loaded, transitions) = lanes.load(ReleasedLane::Agent).await.unwrap().unwrap();
    assert_eq!(loaded.active.image, image('b'));
    assert_eq!(transitions, vec![transition]);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_transitions_are_bounded() {
    let (client, name, lanes) = lanes().await;
    let mut record = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image('0'), "t0").unwrap();
    lanes.bootstrap(record.clone()).await.unwrap();
    let attested = record
        .attest(FleetAttestation {
            generation: 1,
            template_hash: "hash-1".into(),
            attested_at: "t".into(),
        })
        .unwrap();
    lanes
        .replace(&record, attested.clone(), None, None)
        .await
        .unwrap();
    record = attested;
    let digits = "123456789abcdef0123";
    for (index, digit) in digits.chars().enumerate() {
        let observation = format!("obs-{index}");
        let prepared = ready(&record, digit, &observation);
        lanes
            .replace(&record, prepared.clone(), None, None)
            .await
            .unwrap();
        let (promoted, transition) = prepared
            .promote(&evidence(&prepared, digit, &observation), "t")
            .unwrap();
        lanes
            .replace(&prepared, promoted.clone(), Some(transition), None)
            .await
            .unwrap();
        record = promoted;
    }
    let (_, transitions) = lanes.load(ReleasedLane::Agent).await.unwrap().unwrap();
    assert_eq!(transitions.len(), TRANSITION_HISTORY);
    assert_eq!(
        transitions.last().unwrap().to_revision,
        record.active.revision
    );
    client.database(&name).drop().await.unwrap();
}
