use super::*;
use crate::store::agent_log_segments::{self as segments, mongo_tests::fixtures};
use crate::store::mongo_read_telemetry as telemetry;
use crate::store::sandbox_mongo::start_transaction;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::Client;

async fn fixture(
    closed: i64,
    width: i64,
    active: i64,
) -> (Client, String, MongoSandboxStore, Agent) {
    let (client, name) = fixtures::database("cmd_points").await;
    segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = fixtures::agent();
    let entries = (1..=closed * width + active)
        .map(|seq| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: seq as u64,
            appended_at: agent.created_at.clone(),
            kind: if seq == 1 {
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.worktree_id.clone(),
                }
            } else {
                AgentLogKind::ModeChanged {
                    mode: "x".repeat(400),
                }
            },
        })
        .collect();
    let (agent, _) = fixtures::install(&store, agent, entries, closed, width).await;
    (client, name, store, agent)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn command_points_read_one_active_tail_beyond_both_history_limits_without_writes() {
    let (client, name, store, agent) = fixture(4, 3000, 200).await;
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let head = read_head(&store, &row, &mut session, false).await.unwrap();
    assert!(head.last_seq > 10_128 && head.lifetime_bytes > 4 * 1024 * 1024);
    let selected = [
        Some(3),
        Some(3001),
        Some(9000),
        Some(12001),
        Some(12003),
        Some(12100),
        Some(12200),
    ];
    let rows = telemetry::isolated_test(async {
        let rows = points::read(&store, &head, selected, &mut session)
            .await
            .unwrap();
        let [operations, failures, documents, bytes] = telemetry::snapshot("agent_log");
        assert!(operations <= 4, "per-point active validation: {operations}");
        assert_eq!(failures, 0);
        assert!(
            documents <= 203,
            "read work grew with history or point count: {documents}"
        );
        assert!(bytes <= head.active_bytes as u64 + 8192);
        rows
    })
    .await;
    assert_eq!(
        rows.keys().copied().collect::<Vec<_>>(),
        selected.into_iter().flatten().collect::<Vec<_>>()
    );
    assert!(rows.values().all(|entry| entry.agent_id == agent.id));
    session.commit_transaction().await.unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap(),
        Some(row)
    );
    assert_eq!(
        store.agent_log.count_documents(doc! {}).await.unwrap(),
        head.last_seq as u64
    );
    let retained = segments::collections::collection(&store, segments::HEADS)
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Head::read(retained).unwrap(), head);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn command_points_refuse_touched_seal_corruption_and_unselected_active_gaps_without_repair() {
    for sealed in [false, true] {
        let (client, name, store, agent) = fixture(2, 4, 5).await;
        let filter = doc! {"agent_id":agent.id.as_str(),"seq":if sealed {2_i64} else {11_i64}};
        if sealed {
            store
                .agent_log
                .update_one(
                    filter.clone(),
                    doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
                )
                .await
                .unwrap();
        } else {
            store.agent_log.delete_one(filter.clone()).await.unwrap();
        }
        let before = store.agent_log.find_one(filter.clone()).await.unwrap();
        let row = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let head = read_head(&store, &row, &mut session, false).await.unwrap();
        assert!(points::read(
            &store,
            &head,
            [Some(2), Some(9), Some(13), None, None, None, None],
            &mut session
        )
        .await
        .is_err());
        session.abort_transaction().await.unwrap();
        assert_eq!(store.agent_log.find_one(filter).await.unwrap(), before);
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap(),
            Some(row)
        );
        client.database(&name).drop().await.unwrap();
    }
}
