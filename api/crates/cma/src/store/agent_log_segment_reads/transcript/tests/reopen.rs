use super::*;
use layer2_core::sandbox::{
    store::{AgentStore, SandboxStore},
    RetainedReopenRequest,
};

async fn retained(closed: i64) -> (mongodb::Database, MongoSandboxStore, RetainedReopenRequest) {
    let (client, name) = segments::mongo_tests::fixtures::database("segmented_reopen").await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut request =
        crate::store::retained_reopen::mongo_fixtures::reopen_fixture(&store, "sbx_segment_reopen")
            .await;
    let agent = request.snapshot.retained.agents[0].clone();
    let mut entries = store
        .agent_log_prefix(&agent.id, agent.conversation.last_seq)
        .await
        .unwrap();
    while entries.len() < (closed * 16 + 8) as usize {
        entries.push(AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: entries.len() as u64 + 1,
            appended_at: NOW.into(),
            kind: AgentLogKind::ModeChanged {
                mode: "retained".into(),
            },
        });
    }
    store
        .agents
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    let (agent, entries) =
        segments::mongo_tests::fixtures::install(&store, agent, entries, closed, 16).await;
    certify(&store, &agent, &entries).await;
    request.snapshot = store
        .retained_reopen_snapshot(&request.snapshot.retained.sandbox.id)
        .await
        .unwrap();
    (db, store, request)
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_retained_reopen_binds_prefix_and_preserves_original_history() {
    for closed in [1, 10, 100, 1000] {
        let (db, store, request) = retained(closed).await;
        let id = &request.snapshot.retained.sandbox.id;
        let agent = &request.snapshot.retained.agents[0];
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            assert_eq!(
                store.retained_reopen_snapshot(id).await.unwrap(),
                request.snapshot
            );
            let accepted = store.admit_retained_reopen(request.clone()).await.unwrap();
            assert_eq!(
                store.admit_retained_reopen(request.clone()).await.unwrap(),
                accepted
            );
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert!(measured[2] <= 32, "bounded originals: {measured:?}");
        assert!(measured[3] < 32 * 1024, "bounded BSON: {measured:?}");
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().as_ref(),
            Some(agent)
        );
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap(),
            agent.conversation.last_seq
        );
        let row = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
        assert!(row.get_i64("sandbox_recovery_fence").unwrap() > 0);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_retained_reopen_refuses_missing_proof_and_changed_original_without_writes() {
    for fault in ["projection", "creation"] {
        let (db, store, request) = retained(3).await;
        let agent = &request.snapshot.retained.agents[0];
        match fault {
            "projection" => {
                store
                    .segment_logs
                    .projections
                    .delete_many(doc! {})
                    .await
                    .unwrap();
            }
            _ => {
                store
                    .agent_log
                    .update_one(
                        doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
                        doc! {"$set":{"record.appended_at":"2026-01-01T00:00:00Z"}},
                    )
                    .await
                    .unwrap();
            }
        }
        let before = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        assert!(store.admit_retained_reopen(request.clone()).await.is_err());
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap(),
            before
        );
        assert!(store
            .retained_reopen_receipt(&request.snapshot.retained.sandbox.id)
            .await
            .unwrap()
            .is_none());
        db.drop().await.unwrap();
    }
}
