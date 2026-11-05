use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projection_rebuild_is_invisible_until_complete_and_installs_once() {
    let (client, name) = database("segment_rebuild").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, original) = segmented(&store, 3).await;
    let expected = store.agent_log_projection(&agent).await.unwrap().unwrap();
    let filter = doc! {"_id":agent.id.as_str()};
    store
        .segment_logs
        .projections
        .delete_one(filter.clone())
        .await
        .unwrap();
    assert!(store.agent_log_projection(&agent).await.is_err());
    for outcome in ["running", "running", "running"] {
        assert_eq!(
            segments::audit_unit(&client, &db, agent.id.as_str(), "rebuild", false)
                .await
                .unwrap(),
            outcome
        );
        assert!(matches!(
            segments::install_projection(&client, &db, agent.id.as_str(), "rebuild").await,
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(
            store
                .segment_logs
                .heads
                .find_one(filter.clone())
                .await
                .unwrap()
                .unwrap()
                .get_str("evidence_generation")
                .unwrap(),
            "initial"
        );
        assert!(store.agent_log_projection(&agent).await.is_err());
    }
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "rebuild", false)
            .await
            .unwrap(),
        "complete"
    );
    for _ in 0..2 {
        assert_eq!(
            segments::install_projection(&client, &db, agent.id.as_str(), "rebuild")
                .await
                .unwrap(),
            "installed"
        );
        assert_eq!(
            store.agent_log_projection(&agent).await.unwrap(),
            Some(expected.clone())
        );
    }
    let head = store
        .segment_logs
        .heads
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.get_i64("revision").unwrap(), 2);
    assert_eq!(
        head.get_str("evidence_generation").unwrap(),
        "audit:rebuild"
    );
    assert_eq!(
        store.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
        original
    );
    // A retry reconciles the completed installation even after later metadata
    // advancement; it must not publish another generation or reset the head.
    store
        .segment_logs
        .heads
        .update_one(filter.clone(), doc! {"$inc":{"revision":1_i64}})
        .await
        .unwrap();
    store
        .agents
        .update_one(
            filter.clone(),
            doc! {"$inc":{"agent_log_head_revision":1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        segments::install_projection(&client, &db, agent.id.as_str(), "rebuild")
            .await
            .unwrap(),
        "installed"
    );
    assert_eq!(
        store
            .segment_logs
            .heads
            .find_one(filter)
            .await
            .unwrap()
            .unwrap()
            .get_i64("revision")
            .unwrap(),
        3
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_or_cancelled_rebuild_never_replaces_the_live_projection() {
    let (client, name) = database("segment_rebuild_stale").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 1).await;
    let expected = store.agent_log_projection(&agent).await.unwrap();
    for outcome in ["running", "complete"] {
        assert_eq!(
            segments::audit_unit(&client, &db, agent.id.as_str(), "stale", false)
                .await
                .unwrap(),
            outcome
        );
    }
    let filter = doc! {"_id":agent.id.as_str()};
    store
        .segment_logs
        .heads
        .update_one(filter.clone(), doc! {"$inc":{"revision":1_i64}})
        .await
        .unwrap();
    store
        .agents
        .update_one(
            filter.clone(),
            doc! {"$inc":{"agent_log_head_revision":1_i64}},
        )
        .await
        .unwrap();
    store
        .segment_logs
        .projections
        .update_one(filter.clone(), doc! {"$inc":{"head_revision":1_i64}})
        .await
        .unwrap();
    assert!(matches!(
        segments::install_projection(&client, &db, agent.id.as_str(), "stale").await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        segments::audit_unit(&client, &db, agent.id.as_str(), "cancelled", true)
            .await
            .unwrap(),
        "cancelled"
    );
    assert!(matches!(
        segments::install_projection(&client, &db, agent.id.as_str(), "cancelled").await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(store.agent_log_projection(&agent).await.unwrap(), expected);
    assert_eq!(
        store
            .segment_logs
            .heads
            .find_one(filter)
            .await
            .unwrap()
            .unwrap()
            .get_str("evidence_generation")
            .unwrap(),
        "initial"
    );
    db.drop().await.unwrap();
}
