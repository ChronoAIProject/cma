use super::*;
use crate::sandbox::binding_lifecycle_test_support::{
    AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, EmptyAgentSkillResolver,
};
use crate::sandbox::LiveAgentRegistry;
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_worker_records_only_read_only_inspection_observations() {
    exercise_worker(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn repeated_failure_worker_preserves_connection_pacing() {
    exercise_worker(true).await;
}

async fn exercise_worker(single_marker: bool) {
    let (client, name) = database("failure_pacing_worker").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let agent = ready_conversation_fixture(&store, "worker_observation").await;
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let expected = worktree.revision;
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.base_commit = workspace.resolved_commit.unwrap();
    worktree.revision += 1;
    store.save_worktree(expected, worktree).await.unwrap();
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
            registry,
        },
        AgentWorkerConfig::new("inspection-worker", "inspection-test")
            .with_retry_single_marker(single_marker),
    );
    let mut at = NOW.to_owned();
    for _ in 0..2 {
        assert!(worker.run_one(&agent.id, as_time(&at)).await.unwrap());
        at = store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .ready_at;
    }
    let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(saved.work.attempts, 2);
    assert_eq!(
        layer2_core::seconds_from_rfc3339(&until(&store, &agent.id).await).unwrap()
            - layer2_core::seconds_from_rfc3339(&saved.updated_at).unwrap(),
        60
    );
    let logs = store
        .agent_log(&agent.id, agent.conversation.last_seq, 10)
        .await
        .unwrap();
    assert_eq!(logs.len(), if single_marker { 1 } else { 2 });
    assert!(logs.iter().all(|entry| matches!(&entry.kind, AgentLogKind::WorkStepFailed { failure, .. } if failure.class == CLASS)));
    assert!(!worker
        .run_one(&agent.id, as_time(&saved.work.ready_at))
        .await
        .unwrap());
    use layer2_core::sandbox::agent_io::AgentCommandStore;
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    store
        .reserve_command(
            crate::store::sandbox_mongo_integration_tests::command_ledger_tests::request(
                &saved,
                &parent,
                "worker-wake",
            ),
        )
        .await
        .unwrap();
    // The actual worker must capture generation 1 before inspection, then
    // start a fresh streak instead of continually waking on generation 0.
    let wake_at = as_time(&saved.updated_at) + Duration::from_secs(1);
    assert!(worker.run_one(&agent.id, wake_at).await.unwrap());
    let reset = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(until(&store, &agent.id).await, reset.work.ready_at);
    assert!(!worker
        .run_one(&agent.id, wake_at + Duration::from_secs(1))
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}

fn as_time(stamp: &str) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(layer2_core::seconds_from_rfc3339(stamp).unwrap())
}
