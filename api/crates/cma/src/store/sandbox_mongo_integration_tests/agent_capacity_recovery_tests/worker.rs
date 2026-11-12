use super::*;
use crate::sandbox::binding_lifecycle_test_support::{
    AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, EmptyAgentSkillResolver,
};
use crate::sandbox::LiveAgentRegistry;
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use std::time::{Duration, SystemTime};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn parent_pacing_worker_flag_only_changes_parent_observation_path() {
    let (client, name) = database("capacity_pacing_worker").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    for enabled in [false, true] {
        let agent = ready_conversation_fixture(&store, if enabled { "on" } else { "off" }).await;
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
        let revision = worktree.revision;
        worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
        worktree.base_commit = workspace.resolved_commit.unwrap();
        worktree.revision += 1;
        store.save_worktree(revision, worktree).await.unwrap();
        // Coherent existing parent failure; no runtime can be attached by the
        // empty backend. The enabled path must observe readiness before IO.
        let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        parent.status = SandboxStatus::Failed;
        parent.failure = Some(TypedFailure {
            class: "fixture_parent_failed".into(),
            detail: "fixture".into(),
        });
        parent.revision += 1;
        store
            .sandboxes
            .update_one(
                doc! {"_id": parent.id.as_str()},
                doc! {"$set":
                    crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()
                },
            )
            .await
            .unwrap();
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
            AgentWorkerConfig::new("capacity-worker", "capacity-test")
                .with_parent_not_ready_pacing(enabled),
        );
        let at = SystemTime::UNIX_EPOCH
            + Duration::from_secs(layer2_core::seconds_from_rfc3339(NOW).unwrap());
        assert!(worker.run_one(&agent.id, at).await.unwrap());
        let logs = store
            .agent_log(&agent.id, agent.conversation.last_seq, 10)
            .await
            .unwrap();
        assert_eq!(logs.len(), 1);
        let AgentLogKind::WorkStepFailed { step, failure } = &logs[0].kind else {
            panic!("failure observation");
        };
        assert_eq!(*step, AgentWorkStep::PublishConnection);
        if enabled {
            assert_eq!(failure.class, "agent_parent_not_ready");
        } else {
            assert_eq!(failure.class, "agent_runtime_unavailable");
        }
        let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(saved.work.receipts, agent.work.receipts);
        assert_eq!(
            saved.conversation.first_message,
            agent.conversation.first_message
        );
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap().unwrap().status,
            SandboxStatus::Failed
        );
    }
    client.database(&name).drop().await.unwrap();
}
