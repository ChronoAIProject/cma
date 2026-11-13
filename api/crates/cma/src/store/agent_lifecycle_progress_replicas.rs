//! API polling observes durable progress without the writer's local wake hints.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn independent_progress_reader_and_restart_observe_commits_with_owner_isolation() {
    let fixture = Fixture::new().await;
    // Fixture.db deliberately belongs to a different Client from the writer.
    let reader = MongoAgentLifecycleStore::open(&fixture.db).await.unwrap();
    let owner = SubjectId::new("owner");
    let initial = reader
        .read_for_owner(&owner, &fixture.lease.id)
        .await
        .unwrap()
        .unwrap();
    assert!(initial.events.is_empty());
    for (index, observations) in OBSERVATIONS.chunks(2).enumerate() {
        fixture
            .store
            .record_progress_many(&fixture.lease, observations)
            .await
            .unwrap();
        let observed = reader
            .read_for_owner(&owner, &fixture.lease.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.events.len(), (index + 1) * 2);
        assert_eq!(
            observed.events.last().unwrap().sequence,
            ((index + 1) * 2) as u64
        );
        assert_eq!(observed.phase, observations[1].0);
        assert!(reader
            .read_for_owner(&SubjectId::new("other-owner"), &fixture.lease.id)
            .await
            .unwrap()
            .is_none());
    }
    drop(reader);
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let restarted = MongoAgentLifecycleStore::open(&client.database(fixture.db.name()))
        .await
        .unwrap();
    let observed = restarted
        .read_for_owner(&owner, &fixture.lease.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.events.len(), OBSERVATIONS.len());
    assert_eq!(observed.events.last().unwrap().status, "ready");
    assert!(restarted
        .read_for_owner(&owner, "missing-operation")
        .await
        .unwrap()
        .is_none());
    // An unavailable/corrupt shared result must not reuse the preceding success.
    fixture.set(doc! {"events": "invalid-history"}).await;
    assert!(matches!(
        restarted.read_for_owner(&owner, &fixture.lease.id).await,
        Err(StoreError::Unavailable(_))
    ));
    fixture.db.drop().await.unwrap();
}
