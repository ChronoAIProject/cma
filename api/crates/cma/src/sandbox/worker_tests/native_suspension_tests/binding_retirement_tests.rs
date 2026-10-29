use super::*;
use crate::sandbox::{
    agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies},
    workspace_worker::{WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies},
};
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::binding::*;
use layer2_core::sandbox::store::{AgentControlStore, AgentControlTarget, AgentStopRequest};
use layer2_core::sandbox::{AgentStopReason, AgentWorkStep};
use layer2_core::{Caller, Capability};
use serde_json::json;

#[tokio::test]
async fn paused_binding_delete_removes_retained_runtime_without_reopening_agent() {
    for revive_first in [false, true] {
        let mut f = Fixture::native().await;
        f.harness.adapter = f.adapter(&runtime_image());
        let bindings = super::super::binding_tests::service(
            &f.harness,
            Arc::new(InMemoryProfileStore::default()),
        );
        let caller = Caller::new(
            SubjectId::new("manager"),
            Authorization::new([
                Capability::ManageGatewayRecords,
                Capability::ManageProviderBindings,
            ]),
        );
        let profile = json!({"schema_version":1,"metadata":{"name":"paused-binding","owner":{"kind":"human","subject":"owner"},"provider":"cma_codex"},
            "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
            "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
        let digest = format!("sha256:{}", content_digest(&profile).unwrap());
        let key =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        let binding = bindings
            .put(&caller, "paused-binding", profile, &digest, &key)
            .await
            .unwrap();
        bindings.reconcile(20).await.unwrap();
        let created = f.harness.stored(&binding.sandbox_id).await;
        let mut now = start_time(&created);
        let active = f
            .drive(
                &created.id,
                &mut now,
                SandboxStatus::Active,
                &runtime_image(),
            )
            .await;
        let workspace_worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: f.harness.store.clone(),
                adapter: f.harness.adapter.clone(),
            },
            WorkspaceWorkerConfig::new("paused-binding-workspace", "test"),
        );
        for _ in 0..15 {
            bindings.reconcile(20).await.unwrap();
            now += Duration::from_secs(6);
            workspace_worker.run_due(now).await.unwrap();
            if bindings
                .read(&caller, "paused-binding")
                .await
                .unwrap()
                .state
                == BindingState::Ready
            {
                break;
            }
        }
        assert_eq!(
            bindings
                .read(&caller, "paused-binding")
                .await
                .unwrap()
                .state,
            BindingState::Ready
        );
        let (conversation, _) = bindings
            .ensure_conversation(&caller, "paused-binding", "paused-conversation")
            .await
            .unwrap();
        let agent_worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: f.harness.store.clone(),
                adapter: f.harness.adapter.clone(),
                skills: Arc::new(crate::sandbox::agent_skills::EmptyAgentSkillResolver),
                registry: f.harness.registry.clone(),
                updates: DurableAgentUpdateRecorder::new(f.harness.store.clone()),
                operations: AgentOperationService::new(
                    f.harness.store.clone(),
                    f.harness.registry.clone(),
                ),
            },
            AgentWorkerConfig::new("paused-binding-agent", "test"),
        );
        for _ in 0..20 {
            bindings.reconcile(20).await.unwrap();
            now += Duration::from_secs(6);
            agent_worker
                .run_one(&conversation.agent_id, now)
                .await
                .unwrap();
            let current = f
                .harness
                .store
                .get_agent(&conversation.agent_id)
                .await
                .unwrap()
                .unwrap();
            if current.status == AgentStatus::Ready && current.work.step == AgentWorkStep::None {
                break;
            }
        }
        assert_eq!(
            f.harness
                .store
                .get_agent(&conversation.agent_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Ready
        );
        f.harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                active.id.clone(),
                active.owner.clone(),
                rfc3339_at(now),
                None,
            ))
            .await
            .unwrap();
        for _ in 0..20 {
            if f.harness.stored(&active.id).await.status == SandboxStatus::Paused {
                break;
            }
            now += Duration::from_secs(6);
            f.step(&active.id, now, &runtime_image()).await;
        }
        let paused = f.harness.stored(&active.id).await;
        assert_eq!(paused.status, SandboxStatus::Paused, "{:?}", paused);
        let agent = f
            .harness
            .store
            .get_agent(&conversation.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(agent.status, AgentStatus::Paused);
        assert!(paused.runtime.is_none());
        assert!(paused.lifecycle.suspended_runtime.is_some());
        let requests_before = f.harness.backend.requests().await.len();
        // Reproduce an already pending BindingRetired stop admitted by the older release.
        f.harness
            .store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::BindingRetired,
                requested_at: rfc3339_at(now),
            })
            .await
            .unwrap();
        agent_worker.run_one(&agent.id, now).await.unwrap();
        assert_eq!(
            f.harness
                .store
                .get_agent(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .work
                .step,
            AgentWorkStep::CloseProcess
        );
        if revive_first {
            f.harness
                .store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                    paused.id.clone(),
                    paused.owner.clone(),
                    rfc3339_at(now),
                    SandboxReviveCause::ExplicitRequest,
                ))
                .await
                .unwrap();
        }
        bindings.retire(&caller, "paused-binding").await.unwrap();
        bindings.reconcile(20).await.unwrap();
        assert_eq!(
            f.harness.stored(&created.id).await.status,
            SandboxStatus::Retiring
        );
        assert!(matches!(
            f.harness
                .store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                    paused.id.clone(),
                    paused.owner.clone(),
                    rfc3339_at(now),
                    SandboxReviveCause::ExplicitRequest,
                ))
                .await
                .unwrap(),
            layer2_core::sandbox::store::SandboxLifecycleCommandResult::NotEligible(_)
        ));
        for _ in 0..40 {
            now += Duration::from_secs(6);
            let parent = f.harness.stored(&created.id).await;
            if parent.status != SandboxStatus::Retired {
                f.step(&created.id, now, &runtime_image()).await;
            }
            bindings.reconcile(20).await.unwrap();
            agent_worker.run_one(&agent.id, now).await.unwrap();
            if bindings.read(&caller, "paused-binding").await.unwrap().work == BindingWork::Idle {
                break;
            }
        }
        let terminal = bindings.read(&caller, "paused-binding").await.unwrap();
        assert_eq!(terminal.state, BindingState::Retired);
        assert_eq!(terminal.work, BindingWork::Idle);
        assert_eq!(
            f.harness.stored(&created.id).await.status,
            SandboxStatus::Retired
        );
        let deleted = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(deleted.status, AgentStatus::Deleted);
        assert_eq!(deleted.stop_reason, Some(AgentStopReason::BindingRetired));
        assert_eq!(
            deleted.conversation.conversation_id,
            agent.conversation.conversation_id
        );
        assert_eq!(deleted.work.receipts, agent.work.receipts);
        assert!(f
            .adapter(&runtime_image())
            .runtime_inventory()
            .await
            .unwrap()
            .is_empty());
        let requests = f.harness.backend.requests().await;
        assert!(requests[requests_before..]
            .iter()
            .all(|(_, r)| !matches!(r, RequestFrame::OpenAgent(_) | RequestFrame::CloseAgent(_))));
        assert_eq!(
            f.backend
                .calls
                .snapshot()
                .iter()
                .filter(|call| *call == "create")
                .count(),
            1
        );
    }
}
