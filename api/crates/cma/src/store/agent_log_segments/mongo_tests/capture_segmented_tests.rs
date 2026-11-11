use super::*;
use segments::capture::{
    capture, capture_transaction, points, points_transaction, CaptureMode, CapturedFormat,
};
use segments::transaction_scope::TransactionScope;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_capture_scans_active_once_and_points_scale_with_requested_source() {
    let (client, _) = database("unused").await;
    let mut baseline = None;
    for closed in [1, 10, 100, 1000] {
        let name = format!(
            "cma_segments_capture_scale_{}",
            uuid::Uuid::new_v4().simple()
        );
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = segmented(&store, closed).await;
        certify(&store, &agent, &entries).await;
        let mut session = client.start_session().snapshot(true).await.unwrap();
        let consumed = crate::store::mongo_read_telemetry::isolated_test(async {
            let proof = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
                .await
                .unwrap();
            assert_eq!(proof.format(), CapturedFormat::Segmented);
            assert_eq!(proof.latest_entry(), entries.last());
            assert_eq!(proof.projection().through_seq, agent.conversation.last_seq);
            assert_eq!(proof.max_opened_generation(), 0);
            assert_eq!(proof.max_retired_generation(), 0);
            assert_eq!(
                points(
                    &store,
                    &proof,
                    &[1, 2, agent.conversation.last_seq],
                    &mut session,
                    CaptureMode::Snapshot
                )
                .await
                .unwrap(),
                vec![
                    entries[0].clone(),
                    entries[1].clone(),
                    entries.last().unwrap().clone()
                ]
            );
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert_eq!(&consumed[..3], &[2, 0, 5]);
        if let Some(expected) = baseline {
            assert_eq!(consumed, expected);
        } else {
            baseline = Some(consumed);
        }
        drop(session);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_transaction_capture_requires_projection_and_binds_original_points() {
    let (client, name) = database("capture_segment_txn").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented_with_width(&store, 2, 2, 0).await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(capture_transaction(&store, &agent, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    certify(&store, &agent, &entries).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let proof = capture_transaction(&store, &agent, &mut scope)
        .await
        .unwrap();
    assert!(proof.active_leaf_digests().bytes.is_empty());
    assert_eq!(proof.latest_entry(), entries.last());
    assert_eq!(
        points_transaction(&store, &proof, &[1, 3, 4], &mut scope)
            .await
            .unwrap(),
        vec![entries[0].clone(), entries[2].clone(), entries[3].clone()]
    );
    scope
        .update_one(
            &store.segment_logs.heads,
            doc! {"_id":agent.id.as_str()},
            doc! {"$inc":{"revision":1_i64}},
        )
        .await
        .unwrap();
    assert!(points_transaction(&store, &proof, &[1], &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    store
        .segment_logs
        .projections
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$unset":{"record.max_opened_generation":""}},
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .is_err());
    drop(session);
    db.drop().await.unwrap();
}
