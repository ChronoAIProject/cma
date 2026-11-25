use super::*;

#[path = "agent_lifecycle_progress_test_support.rs"]
mod support;
use support::Fixture;
#[path = "agent_lifecycle_progress_race_tests.rs"]
mod races;
#[path = "agent_lifecycle_progress_replicas.rs"]
mod replicas;

const OBSERVATIONS: [(&str, &str, &str); 8] = [
    ("sandbox_ready", "running", "Preparing the Agent Sandbox"),
    ("sandbox_ready", "ready", "Agent Sandbox is ready"),
    (
        "key_scope_ready",
        "running",
        "Configuring Agent key permissions",
    ),
    (
        "key_scope_ready",
        "ready",
        "Agent key permissions are ready",
    ),
    (
        "workspace_runtime",
        "running",
        "Preparing an empty workspace",
    ),
    ("workspace_runtime", "ready", "Workspace is ready"),
    ("agent_startup", "running", "Starting your Agent"),
    ("agent_startup", "ready", "Your Agent is ready"),
];

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn default_progress_pairs_preserve_eight_events_with_ten_mongo_commands() {
    let fixture = Fixture::new().await;
    let accepted = fixture.read().await.created_at;
    fixture.probe.take_commands();
    for observations in [
        &OBSERVATIONS[..1],
        &OBSERVATIONS[1..3],
        &OBSERVATIONS[3..5],
        &OBSERVATIONS[5..7],
        &OBSERVATIONS[7..],
    ] {
        fixture
            .store
            .record_progress_many(&fixture.lease, observations)
            .await
            .unwrap();
    }
    let commands = fixture.probe.take_commands();
    assert_eq!(
        commands.len(),
        10,
        "five reads and five majority writes replace sixteen requests"
    );
    for pair in commands.chunks_exact(2) {
        assert_eq!(pair[0].0, "find");
        assert_eq!(pair[1].0, "update");
        assert_eq!(
            pair[0]
                .1
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "majority"
        );
        assert_eq!(
            pair[1]
                .1
                .get_document("writeConcern")
                .unwrap()
                .get_str("w")
                .unwrap(),
            "majority"
        );
    }
    let record = fixture.read().await;
    assert_eq!(
        record.view().accepted_at,
        Some(accepted.try_to_rfc3339_string().unwrap())
    );
    assert_eq!(record.status, "running");
    assert_eq!(record.phase, "agent_startup");
    assert_eq!(record.events.len(), 8);
    for (index, event) in record.events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
        assert_eq!(
            (
                event.phase.as_str(),
                event.status.as_str(),
                event.message.as_str()
            ),
            OBSERVATIONS[index]
        );
        assert!(!event.occurred_at.is_empty());
    }
    let before = serde_json::to_value(&record.events).unwrap();
    for observations in OBSERVATIONS.chunks(2) {
        fixture
            .store
            .record_progress_many(&fixture.lease, observations)
            .await
            .unwrap();
    }
    let replay = fixture.read().await;
    assert_eq!(replay.view().accepted_at, record.view().accepted_at);
    assert_eq!(serde_json::to_value(replay.events).unwrap(), before);
    assert_eq!(replay.phase, "agent_startup");
    assert!(fixture
        .probe
        .take_commands()
        .iter()
        .all(|(name, _)| name == "find"));
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn partial_and_legacy_replays_preserve_second_event_without_readiness_regression() {
    let fixture = Fixture::new().await;
    fixture
        .store
        .append_event(&fixture.lease, 4, "sandbox_ready", "ready", "Legacy ready")
        .await
        .unwrap();
    fixture
        .store
        .record_progress_many(&fixture.lease, &OBSERVATIONS[1..3])
        .await
        .unwrap();
    let partial = fixture.read().await;
    assert_eq!(partial.events.len(), 2);
    assert_eq!(partial.events[0].message, "Legacy ready");
    assert_eq!(partial.events[1].sequence, 5);
    assert_eq!(partial.events[1].phase, "key_scope_ready");
    fixture
        .store
        .record_progress_many(&fixture.lease, &[OBSERVATIONS[3], OBSERVATIONS[2]])
        .await
        .unwrap();
    let ready = fixture.read().await;
    assert_eq!(ready.events.len(), 3);
    assert_eq!(ready.events.last().unwrap().status, "ready");
    fixture
        .store
        .record_progress_many(&fixture.lease, &[OBSERVATIONS[2], OBSERVATIONS[2]])
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(fixture.read().await.events).unwrap(),
        serde_json::to_value(ready.events).unwrap()
    );
    fixture
        .store
        .record_progress_many(
            &fixture.lease,
            &[
                ("fresh", "ready", "Done"),
                ("fresh", "running", "Late replay"),
            ],
        )
        .await
        .unwrap();
    let within_batch = fixture.read().await;
    assert_eq!(within_batch.events.len(), 4);
    assert_eq!(within_batch.events.last().unwrap().status, "ready");
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn progress_retention_batch_size_and_sequence_overflow_are_bounded() {
    let fixture = Fixture::new().await;
    for index in 0..18 {
        let phase = format!("phase-{index}");
        fixture
            .store
            .record_progress_many(
                &fixture.lease,
                &[(&phase, "running", "Start"), (&phase, "ready", "Done")],
            )
            .await
            .unwrap();
    }
    let record = fixture.read().await;
    assert_eq!(record.events.len(), MAX_EVENTS as usize);
    assert_eq!(
        record
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (5..=36).collect::<Vec<_>>()
    );
    assert_eq!(record.phase, "phase-17");
    fixture.probe.take_commands();
    for invalid in [&OBSERVATIONS[..0], &OBSERVATIONS[..3]] {
        assert!(matches!(
            fixture
                .store
                .record_progress_many(&fixture.lease, invalid)
                .await,
            Err(StoreError::Malformed(_))
        ));
    }
    assert!(fixture.probe.take_commands().is_empty());
    let mut events = record.events;
    events.last_mut().unwrap().sequence = i64::MAX as u64;
    fixture
        .set(doc! {"events": mongodb::bson::serialize_to_bson(&events).unwrap()})
        .await;
    assert!(matches!(
        fixture
            .store
            .record_progress_many(&fixture.lease, &OBSERVATIONS[..2])
            .await,
        Err(StoreError::Malformed(_))
    ));
    assert_eq!(
        fixture.read().await.events.last().unwrap().sequence,
        i64::MAX as u64
    );
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn progress_store_errors_propagate_and_retries_append_no_partial_batch() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .run_command(doc! {"collMod": COLLECTION, "validator": {"phase": {"$ne": "sandbox_ready"}}})
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .record_progress_many(&fixture.lease, &OBSERVATIONS[..2])
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert!(fixture.read().await.events.is_empty());
    fixture
        .db
        .run_command(doc! {"collMod": COLLECTION, "validator": {}})
        .await
        .unwrap();
    fixture
        .store
        .record_progress_many(&fixture.lease, &OBSERVATIONS[..2])
        .await
        .unwrap();
    assert_eq!(fixture.read().await.events.len(), 2);
    fixture.set(doc! {"events": "corrupt history"}).await;
    assert!(matches!(
        fixture
            .store
            .record_progress_many(&fixture.lease, &OBSERVATIONS[2..4])
            .await,
        Err(StoreError::Unavailable(_))
    ));
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn existing_single_event_recovery_and_ownership_contract_remains_valid() {
    let fixture = Fixture::new().await;
    fixture
        .db
        .collection::<mongodb::bson::Document>(COLLECTION)
        .delete_one(doc! {"_id": &fixture.lease.id})
        .await
        .unwrap();
    crate::store::isolation::agent_lifecycle_contract::verify(&fixture.db).await;
    fixture.db.drop().await.unwrap();
}
