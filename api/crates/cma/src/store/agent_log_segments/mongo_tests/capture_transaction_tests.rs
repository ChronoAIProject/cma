use super::capture_tests::legacy;
use super::*;
use segments::{
    capture::{capture, capture_transaction, points_transaction, CaptureMode},
    transaction_scope::TransactionScope,
};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn captured_transaction_proof_rejects_later_transactions_after_abort_and_commit() {
    for committed in [false, true] {
        let (client, name) = database("capture_transaction").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = legacy(&store, vec![]).await;
        let mut session = client.start_session().await.unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        let captured = capture_transaction(&store, &agent, &mut scope)
            .await
            .unwrap();
        assert_eq!(
            points_transaction(&store, &captured, &[1], &mut scope)
                .await
                .unwrap(),
            entries
        );
        let released = scope.release();
        if committed {
            released.commit_transaction().await.unwrap();
        } else {
            released.abort_transaction().await.unwrap();
        }
        let mut later = TransactionScope::start(&mut session).await.unwrap();
        crate::store::mongo_read_telemetry::isolated_test(async {
            assert!(points_transaction(&store, &captured, &[1], &mut later)
                .await
                .is_err());
            assert_eq!(
                crate::store::mongo_read_telemetry::snapshot("agent_log"),
                [0; 4]
            );
        })
        .await;
        let current = capture_transaction(&store, &agent, &mut later)
            .await
            .unwrap();
        assert_eq!(
            points_transaction(&store, &current, &[1], &mut later)
                .await
                .unwrap(),
            entries
        );
        later.release().abort_transaction().await.unwrap();
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn captured_proofs_cannot_cross_session_database_or_snapshot_transaction_modes() {
    let (client, name) = database("capture_scope").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let other_name = format!("{name}_other");
    let other_store = MongoSandboxStore::open(&client, &other_name).await.unwrap();
    let (agent, _) = legacy(&store, vec![]).await;
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    let snapshot_proof = capture(&store, &agent, &mut snapshot, CaptureMode::Snapshot)
        .await
        .unwrap();
    let mut first = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut first).await.unwrap();
    assert!(
        points_transaction(&store, &snapshot_proof, &[1], &mut scope)
            .await
            .is_err()
    );
    let captured = capture_transaction(&store, &agent, &mut scope)
        .await
        .unwrap();
    assert!(
        points_transaction(&other_store, &captured, &[1], &mut scope)
            .await
            .is_err()
    );
    let mut second = client.start_session().await.unwrap();
    let mut other = TransactionScope::start(&mut second).await.unwrap();
    assert!(points_transaction(&store, &captured, &[1], &mut other)
        .await
        .is_err());
    other.release().abort_transaction().await.unwrap();
    scope
        .update_one(
            &store.agents,
            doc! {"_id":agent.id.as_str()},
            doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
        )
        .await
        .unwrap();
    assert!(points_transaction(&store, &captured, &[1], &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    drop(snapshot);
    client.database(&name).drop().await.unwrap();
    client.database(&other_name).drop().await.unwrap();
}
