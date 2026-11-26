use super::*;
use crate::store::{
    agent_log::log_document, agents::agent_document, sandbox_mongo::start_transaction,
    sandbox_mongo_integration_tests::resource_access_tests::fixture,
};
use layer2_core::{
    resource_access::ResourceAccessStore,
    sandbox::{AgentLogEntry, HierarchyReadStore},
    WorktreeId,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

async fn observed() -> (Client, String, Arc<Mutex<Vec<String>>>) {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command_name);
        }
    }));
    (
        Client::with_options(options).unwrap(),
        format!("cma_task_batch_{}", uuid::Uuid::new_v4().simple()),
        trace,
    )
}

async fn log(store: &MongoSandboxStore, agent: &Agent, seq: u64, kind: AgentLogKind) {
    store
        .agent_log
        .insert_one(
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq,
                appended_at: agent.created_at.clone(),
                kind,
            })
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn examined(client: &Client, name: &str, agent: &Agent) -> i64 {
    client
        .database(name)
        .run_command(doc! {
            "explain": {"find": "agent_log", "filter": {
                "agent_id": agent.id.as_str(), "record.kind.type": "conversation_gap",
                "seq": {"$gt": 1_i64, "$lt": 1286_i64},
            }, "sort": {"seq": -1}, "limit": 1}, "verbosity": "executionStats",
        })
        .await
        .unwrap()
        .get_document("executionStats")
        .unwrap()
        .get_i32("totalDocsExamined")
        .unwrap() as i64
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_status_batch_matches_sequential_evidence_and_bounds_round_trips() {
    let (client, name, trace) = observed().await;
    let (store, target) = fixture(&client, &name).await;
    let original = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let mut agents = Vec::new();
    for index in 0..20 {
        let mut agent = original.clone();
        agent.id = AgentId(format!("agt_status_{index:02}"));
        agent.creation_key = format!("status_{index}");
        agent.worktree_id = WorktreeId(format!("wkt_status_{index}"));
        agent.conversation.last_seq = 1286;
        store
            .agents
            .insert_one(agent_document(&agent).unwrap())
            .await
            .unwrap();
        let logs: Vec<_> = (1..=1286)
            .map(|seq| {
                let kind = if seq == 1 {
                    AgentLogKind::TurnStarted {
                        turn_sequence: 1,
                        model: None,
                        origin_key: None,
                    }
                } else if seq == 1286 {
                    AgentLogKind::TurnCompleted { turn_sequence: 1 }
                } else if seq == 1285 && index % 2 == 0 {
                    AgentLogKind::ConversationGap { frames: 1 }
                } else {
                    AgentLogKind::AgentMessage {
                        content: "x".repeat(4096),
                    }
                };
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: agent.created_at.clone(),
                    kind,
                })
                .unwrap()
            })
            .collect();
        store.agent_log.insert_many(logs).await.unwrap();
        agents.push(agent);
    }
    // Measure the original query pattern/index set and the final path against
    // identical retained evidence, with real Mongo command observations.
    store
        .agent_log
        .drop_index("agent_log_task_status")
        .await
        .unwrap();
    let examined_before = examined(&client, &name, &agents[1]).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    trace.lock().unwrap().clear();
    let started = std::time::Instant::now();
    let mut expected = HashMap::new();
    for agent in &agents {
        expected.insert(
            agent.id.clone(),
            super::super::read_with_turn(&store, agent, &mut session)
                .await
                .unwrap(),
        );
    }
    let before_ms = started.elapsed().as_secs_f64() * 1000.0;
    let before = trace.lock().unwrap().len();
    session.abort_transaction().await.unwrap();
    super::super::ensure_indexes(&store.agent_log)
        .await
        .unwrap();
    let examined_after = examined(&client, &name, &agents[1]).await;
    assert_eq!((examined_before, examined_after), (1284, 0));
    start_transaction(&mut session).await.unwrap();
    trace.lock().unwrap().clear();
    let started = std::time::Instant::now();
    let actual: HashMap<_, _> = read_batch(&store, &agents, &mut session, false)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, evidence)| (id, evidence.observation))
        .collect();
    let after_ms = started.elapsed().as_secs_f64() * 1000.0;
    let after = trace.lock().unwrap().clone();
    assert_eq!(actual, expected);
    assert_eq!(before, 60);
    assert_eq!(after, ["aggregate"]);
    assert_eq!(
        actual
            .values()
            .filter(|(status, _)| *status == AgentTaskStatus::Completed)
            .count(),
        10
    );
    assert_eq!(
        actual
            .values()
            .filter(|(status, _)| *status == AgentTaskStatus::Failed)
            .count(),
        10
    );
    session.abort_transaction().await.unwrap();
    println!("20 ready Agents x 1,286 events x 4 KiB: queries {before} -> {}; elapsed {before_ms:.3} -> {after_ms:.3} ms", after.len());
    println!("gap query documents examined: {examined_before} -> {examined_after}");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_status_batch_retains_corruption_large_record_and_snapshot_contracts() {
    let (client, name, trace) = observed().await;
    let (store, target) = fixture(&client, &name).await;
    let mut agent = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    // A status-relevant row can exceed the batching byte cap. Fall back in the
    // same transaction; do not truncate, drop it or fabricate a status.
    log(
        &store,
        &agent,
        3,
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            model: Some("x".repeat(70 * 1024)),
            origin_key: None,
        },
    )
    .await;
    agent.conversation.last_seq = 3;
    store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    start_transaction(&mut session).await.unwrap();
    trace.lock().unwrap().clear();
    let result = read_many(&store, std::slice::from_ref(&agent), &mut session)
        .await
        .unwrap();
    assert_eq!(result[&agent.id], AgentTaskStatus::Working);
    assert_eq!(*trace.lock().unwrap(), ["aggregate", "find", "find"]);
    // A concurrent append does not change this transaction's existing snapshot.
    log(
        &store,
        &agent,
        4,
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    )
    .await;
    assert_eq!(
        read_many(&store, std::slice::from_ref(&agent), &mut session)
            .await
            .unwrap(),
        result
    );
    session.abort_transaction().await.unwrap();
    // Evidence beyond the captured high-water remains Unknown, as before.
    start_transaction(&mut session).await.unwrap();
    assert_eq!(
        read_many(&store, std::slice::from_ref(&agent), &mut session)
            .await
            .unwrap()[&agent.id],
        AgentTaskStatus::Unknown
    );
    session.abort_transaction().await.unwrap();
    store
        .agent_log
        .update_one(
            doc! {"agent_id": agent.id.as_str(), "seq": 4_i64},
            doc! {"$set": {"record.agent_id": "foreign"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.read_workspace_hierarchy(&agent.workspace_id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    agent.status = AgentStatus::Paused;
    start_transaction(&mut session).await.unwrap();
    trace.lock().unwrap().clear();
    assert_eq!(
        read_many(&store, std::slice::from_ref(&agent), &mut session)
            .await
            .unwrap()[&agent.id],
        AgentTaskStatus::Paused
    );
    assert!(trace.lock().unwrap().is_empty());
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}
