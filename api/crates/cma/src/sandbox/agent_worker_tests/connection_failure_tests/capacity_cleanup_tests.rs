use super::super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::{AgentLogEntry, RecordedToolOutput};
use mongodb::bson::{doc, serialize_to_bson, Document};
use mongodb::Client;

mod cpu_profile;
mod direct_retry_tests;
mod exhausted_cleanup_tests;
mod ledger_hold_tests;
mod removal_retry_tests;
mod settlement_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_history_worker_closes_and_deletes_with_original_runtime_effects() {
    cleanup_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn byte_exhausted_worker_closes_and_deletes_with_original_runtime_effects() {
    cleanup_case(true).await;
}

async fn cleanup_case(bytes: bool) {
    let (client, database, harness) = fixture().await;
    let agent = harness.admit("worker-capacity-cleanup", vec![]).await;
    for _ in 0..5 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let current = harness.stored(&agent.id).await;
    let ready =
        crate::store::fill_agent_capacity_fixture(harness.store.as_ref(), current, bytes).await;
    let started = std::time::Instant::now();
    let identity = ready.conversation.clone();
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped.work.lease.is_none());
    assert_eq!(
        stopped.conversation.conversation_id,
        identity.conversation_id
    );
    assert_eq!(
        stopped.conversation.runtime_generation,
        identity.runtime_generation
    );
    assert!(stopped
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    assert!(harness.registry.resolve(&Harness::target(&ready)).is_none());
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        harness.run(&agent.id, SystemTime::now()).await;
    }
    let deleted = harness.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert_eq!(deleted.work.step, AgentWorkStep::None);
    assert!(deleted.work.lease.is_none());
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(!snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(!snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));
    let operations: Vec<_> = harness.backend.requests().await[before..]
        .iter()
        .map(|(_, request)| request.operation())
        .collect();
    assert_eq!(
        operations,
        [
            WireOperation::CloseAgent,
            WireOperation::RemoveAgentHome,
            WireOperation::RemoveWorktree
        ]
    );
    assert_eq!(
        deleted.conversation.last_seq,
        ready.conversation.last_seq + 7
    );
    assert!(started.elapsed() < Duration::from_secs(120));
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(deleted));
    let tail = reopened
        .agent_log(&agent.id, ready.conversation.last_seq - 1, 20)
        .await
        .unwrap();
    assert!(
        matches!(&tail[0].kind, AgentLogKind::ToolOutputReported { output, .. }
        if !output.text.is_empty())
    );
    assert_eq!(tail.last().unwrap().kind, AgentLogKind::Deleted);
    client.database(&database).drop().await.unwrap();
}

async fn fixture() -> (Client, String, Harness<MongoSandboxStore>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    fixture_with_client(client).await
}

async fn fixture_with_client(client: Client) -> (Client, String, Harness<MongoSandboxStore>) {
    let database = format!("cma_worker_capacity_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let mut harness =
        Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
    harness.now = SystemTime::now();
    // Exercise the production lease rather than the short default test lease.
    let config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    assert_eq!(config.lease_duration, Duration::from_secs(18 * 60));
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(store, harness.registry.clone()),
        },
        config,
    );
    (client, database, harness)
}

async fn fill_history(
    client: &Client,
    database: &str,
    harness: &Harness<MongoSandboxStore>,
    id: &AgentId,
) -> Agent {
    let current = harness.stored(id).await;
    let database = client.database(database);
    let entries: Vec<_> = (current.conversation.last_seq + 1..=10_000)
        .map(|seq| {
            let entry = AgentLogEntry {
                agent_id: id.clone(),
                seq,
                appended_at: current.updated_at.clone(),
                kind: AgentLogKind::ToolOutputReported {
                    call_id: "retained-worker-fixture".into(),
                    output: RecordedToolOutput {
                        text: "Retained worker capacity fixture output".into(),
                        truncated: false,
                    },
                },
            };
            doc! {"_id":format!("{id}:{seq:020}"), "document_schema_version":1_i64,
            "agent_id":id.as_str(), "seq":seq as i64, "record":serialize_to_bson(&entry).unwrap()}
        })
        .collect();
    database
        .collection::<Document>("agent_log")
        .insert_many(entries)
        .await
        .unwrap();
    let updated = database
        .collection::<Document>("agents")
        .update_one(
            doc! {"_id":id.as_str(), "revision":current.revision as i64},
            doc! {"$set":{"last_seq":10_000_i64, "record.conversation.last_seq":10_000_i64}},
        )
        .await
        .unwrap();
    assert_eq!(updated.matched_count, 1);
    let ready = harness.stored(id).await;
    assert_eq!(ready.conversation.last_seq, 10_000);
    ready
}
