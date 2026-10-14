use super::cross_interface_tests::{admitted, authenticated, Route};
use super::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::{
    AgentStopRequest, SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore,
};
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, TypedFailure};
use layer3_adapter::{PromptResponse, TurnStopReason};
use tower::ServiceExt;

const OPERATION: &str = "worker-bootstrap";

struct BootstrapHarness {
    harness: Harness,
    ready_parent: Sandbox,
    ready_workspace: Workspace,
}

impl BootstrapHarness {
    async fn new() -> Self {
        let original = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let mut ready_parent = original
            .store
            .get_sandbox(&original.workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        ready_parent.origin = SandboxOrigin::Lifecycle {
            operation_id: OPERATION.into(),
        };
        ready_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
            original.workspace.profile.id.clone(),
            original.workspace.profile.revision,
        )];
        let mut ready_workspace = original.workspace.clone();
        ready_workspace.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
        let mut parent = ready_parent.clone();
        parent.status = SandboxStatus::Provisioning;
        parent.runtime = None;
        parent.credential.current = None;
        parent.credential.delivered = false;
        parent.lifecycle.runtime_generation_high_water = 0;
        parent.work =
            SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, rfc3339_at(original.now));
        let store = Arc::new(InMemorySandboxStore::default());
        store
            .admit_sandbox(
                SandboxAdmission::new(
                    parent.clone(),
                    SandboxAdmissionName::Exact(parent.name.clone()),
                    4,
                    rfc3339_at(original.now),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut workspace = ready_workspace.clone();
        workspace.status = WorkspaceStatus::Preparing;
        workspace.resolved_commit = None;
        workspace.ready_at = None;
        workspace.work =
            WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, rfc3339_at(original.now));
        store.create_workspace(workspace.clone()).await.unwrap();
        let registry = Arc::new(LiveAgentRegistry::default());
        let operations = AgentOperationService::new(store.clone(), registry.clone());
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let mut config = AgentWorkerConfig::new("bootstrap-worker", DEPLOYMENT_ID);
        config.lease_duration = Duration::from_secs(3);
        config.retry_delay = Duration::from_secs(30);
        config.open_timeout = Duration::from_secs(2);
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: original.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: registry.clone(),
                updates: updates.clone(),
                operations,
            },
            config,
        );
        Self {
            harness: Harness {
                store,
                worker,
                registry,
                backend: original.backend,
                adapter: original.adapter,
                runtime: original.runtime,
                workspace,
                now: SystemTime::now(),
                updates,
            },
            ready_parent,
            ready_workspace,
        }
    }

    async fn admit(&self, first: Option<&str>) -> Agent {
        let h = &self.harness;
        let origin = AgentOrigin::Lifecycle {
            operation_id: OPERATION.into(),
        };
        let key = format!(
            "lifecycle:{}",
            layer2_core::profiles::content_digest(&origin).unwrap()
        );
        let stamp = rfc3339_at(h.now);
        let created = h
            .store
            .admit_agent(AgentAdmission {
                binding: None,
                created_by: None,
                owner: h.workspace.owner.clone(),
                workspace_id: h.workspace.id.clone(),
                agent_id: AgentId::generate(),
                worktree_id: WorktreeId::generate(),
                profile: agent_profile(vec![]),
                capabilities: RuntimeCapabilities::new([
                    RuntimeCapability::Conversation,
                    RuntimeCapability::Steer,
                    RuntimeCapability::Interrupt,
                    RuntimeCapability::Usage,
                    RuntimeCapability::Resources,
                ]),
                title: None,
                first_message: first
                    .map(|text| FirstMessageRecord::new(&key, text, &stamp).unwrap()),
                origin,
                creation_key: key,
                event_at: stamp,
            })
            .await
            .unwrap();
        assert_eq!(
            created.creation().agent.work.step,
            AgentWorkStep::WaitForWorkspace
        );
        created.creation().agent.clone()
    }

    async fn ready(&self) {
        let h = &self.harness;
        let current = h
            .store
            .get_workspace(&h.workspace.id)
            .await
            .unwrap()
            .unwrap();
        let mut workspace = self.ready_workspace.clone();
        workspace.next_worktree_number = current.next_worktree_number;
        workspace.revision = current.revision + 1;
        h.store
            .save_workspace(current.revision, workspace)
            .await
            .unwrap();
        let current = h
            .store
            .get_sandbox(&h.workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let mut parent = self.ready_parent.clone();
        parent.revision = current.revision + 1;
        h.store
            .save_sandbox(current.revision, parent)
            .await
            .unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn bootstrap_expected_wait_defers_without_failure_logs_or_runtime_effects() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
    let workspace_worker = crate::sandbox::workspace_worker::WorkspaceWorker::new(
        crate::sandbox::workspace_worker::WorkspaceWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
        },
        crate::sandbox::workspace_worker::WorkspaceWorkerConfig::new(
            "early-workspace",
            DEPLOYMENT_ID,
        ),
    );
    let original_workspace = h
        .store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace_worker.run_due(h.now).await.unwrap(), 0);
    assert_eq!(
        h.store
            .get_workspace(&agent.workspace_id)
            .await
            .unwrap()
            .unwrap(),
        original_workspace
    );
    let started = tokio::time::Instant::now();
    h.run(&agent.id, h.now).await;
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    let waiting = h.stored(&agent.id).await;
    assert_eq!(waiting.status, AgentStatus::Starting);
    assert_eq!(waiting.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(waiting.work.lease.is_none());
    assert_eq!(waiting.work.attempts, 1);
    assert_eq!(
        waiting.work.ready_at,
        rfc3339_at(h.now + Duration::from_secs(2))
    );
    assert!(waiting.failure.is_none());
    assert_eq!(h.store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
    assert_eq!(h.backend.requests().await.len(), before);
    assert!(!h.worker.run_one(&agent.id, h.now).await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn bootstrap_wait_observes_real_readiness_then_startup_effects_run_once() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    tokio::join!(h.run(&agent.id, h.now), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        fixture.ready().await;
    });
    let bound = h.stored(&agent.id).await;
    assert_eq!(bound.work.step, AgentWorkStep::AddWorktree);
    assert_eq!(h.backend.requests().await.len(), before);
    assert_eq!(
        h.store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .base_commit,
        fixture.ready_workspace.resolved_commit.clone().unwrap()
    );
    assert!(h
        .worker
        .run_scheduled(&agent.id, h.now + Duration::from_secs(1))
        .await
        .unwrap());
    let ready = h.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.receipts.len(), 3);
    assert!(ready.work.cleanup_debt.is_empty());
    let snapshot = h.backend.snapshot(&h.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.homes.len(), 1);
    assert!(!h
        .worker
        .run_scheduled(&agent.id, h.now + Duration::from_secs(2))
        .await
        .unwrap());
    assert!(!h
        .store
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::WorkStepFailed { .. })));
}

#[tokio::test(start_paused = true)]
async fn bootstrap_stop_supersedes_wait_and_delete_finishes_without_runtime_effects() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    tokio::join!(h.run(&agent.id, h.now), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let current = h.stored(&agent.id).await;
        h.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&current),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(h.now),
            })
            .await
            .unwrap();
    });
    h.run(&agent.id, h.now + Duration::from_secs(1)).await;
    // Existing Stop semantics retain the superseded lease's cleanup fence.
    assert!(!h
        .worker
        .run_one(&agent.id, h.now + Duration::from_secs(1))
        .await
        .unwrap());
    for _ in 0..2 {
        h.run(&agent.id, h.now + Duration::from_secs(4)).await;
    }
    let stopped = h.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped.has_no_runtime_effects());
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(h.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        h.run(&agent.id, h.now + Duration::from_secs(5)).await;
    }
    let deleted = h.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert!(deleted.has_no_runtime_effects());
    assert_eq!(
        h.store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    assert_eq!(h.backend.requests().await.len(), before);
}

async fn queue(
    h: &Harness,
    agent: &Agent,
    probe: Arc<runtime_tests::RuntimeProbe>,
) -> (AdmittedAgentCommand, Arc<AgentCommandWorker>) {
    let (router, worker) =
        conversation_router_with_probe(h, Some(probe), "bootstrap-command-worker").await;
    let response = authenticated(router, h)
        .oneshot(Route::ResponsesV2.request(&agent.id, "queued followup"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let original = admitted(h).await;
    assert_eq!(
        response.headers()["x-cma-command-id"],
        original.receipt.envelope.command_id.as_str()
    );
    assert_eq!(original.status, AgentResponseStatus::Queued);
    assert!(original.work.dispatch.is_none());
    drop(response);
    (original, worker)
}

#[tokio::test]
async fn bootstrap_v2_queue_survives_wait_and_first_prompt_barrier_then_dispatches_once() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(Some("configured first prompt")).await;
    let probe = runtime_tests::RuntimeProbe::gated();
    let (original, worker) = queue(h, &agent, probe.clone()).await;
    let id = &original.receipt.envelope.command_id;
    let before = h.backend.requests().await.len();
    tokio::time::timeout(Duration::from_secs(2), worker.run_one(id))
        .await
        .expect("fresh queued bootstrap input releases the worker for a prompt recheck")
        .unwrap();
    let waiting = h.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(waiting.receipt, original.receipt);
    assert_eq!(waiting.status, AgentResponseStatus::Queued);
    assert!(waiting.work.dispatch.is_none());
    assert!(probe.inputs().is_empty());
    assert_eq!(h.backend.requests().await.len(), before);
    fixture.ready().await;
    // Stop just before first-message dispatch to exercise its queued barrier.
    // The scheduled worker intentionally drains this next step immediately.
    for _ in 0..6 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let ready = h.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::DispatchFirstMessage);
    worker.run_one(id).await.unwrap();
    assert_eq!(
        h.store.get_command(id).await.unwrap().unwrap().status,
        AgentResponseStatus::Queued
    );
    assert!(probe.inputs().is_empty());
    h.run(&agent.id, SystemTime::now()).await;
    let working = h.stored(&agent.id).await;
    assert_eq!(
        working.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    worker.run_one(id).await.unwrap();
    assert!(probe.inputs().is_empty());
    h.updates
        .record_update(
            &agent.id,
            working.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    worker.run_one(id).await.unwrap();
    let bound = h.store.get_command(id).await.unwrap().unwrap();
    assert_eq!(bound.receipt, original.receipt);
    assert!(bound.work.dispatch.is_some());
    assert_eq!(probe.inputs().len(), 1);
    worker.run_one(id).await.unwrap();
    assert_eq!(probe.inputs().len(), 1);
    probe.release_reply();
}

#[tokio::test]
async fn bootstrap_parent_failure_terminalizes_queued_v2_without_runtime_effects() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let probe = runtime_tests::RuntimeProbe::gated();
    let (original, worker) = queue(h, &agent, probe.clone()).await;
    let before = h.backend.requests().await.len();
    let mut parent = h
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = parent.revision;
    parent
        .transition(
            SandboxStatus::Failed,
            layer2_core::now_rfc3339(),
            Some(TypedFailure {
                class: "fixture_bootstrap_failure".into(),
                detail: "fixture runtime could not start".into(),
            }),
        )
        .unwrap();
    h.store.save_sandbox(revision, parent).await.unwrap();
    worker
        .run_one(&original.receipt.envelope.command_id)
        .await
        .unwrap();
    let terminal = h
        .store
        .get_command(&original.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(terminal.status.is_terminal());
    assert_eq!(terminal.receipt, original.receipt);
    assert!(terminal.work.dispatch.is_none());
    h.run(&agent.id, SystemTime::now()).await;
    let failed = h.stored(&agent.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "agent_parent_failed"
    );
    assert!(failed.has_no_runtime_effects());
    assert!(probe.inputs().is_empty());
    assert_eq!(h.backend.requests().await.len(), before);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_mongo_workspace_claim_defers_without_mutation_until_parent_install_ready() {
    use layer2_core::sandbox::store::WorkspaceWorkerStore;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable local Mongo required");
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_bootstrap_workspace_claim_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = crate::store::MongoSandboxStore::open(&client, db.name())
        .await
        .unwrap();
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let parent = h
        .store
        .get_sandbox(&h.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                parent.clone(),
                SandboxAdmissionName::Exact(parent.name.clone()),
                4,
                rfc3339_at(h.now),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store.create_workspace(h.workspace.clone()).await.unwrap();
    let now = rfc3339_at(h.now);
    let expires = rfc3339_at(h.now + Duration::from_secs(30));
    assert!(store
        .claim_workspace_work(
            &h.workspace.id,
            "workspace-worker",
            now.clone(),
            expires.clone()
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.get_workspace(&h.workspace.id).await.unwrap().unwrap(),
        h.workspace
    );
    let mut installing = parent.clone();
    installing.status = SandboxStatus::Installing;
    installing.runtime = fixture.ready_parent.runtime.clone();
    installing.lifecycle.runtime_generation_high_water =
        fixture.ready_parent.lifecycle.runtime_generation_high_water;
    installing.revision += 1;
    store
        .save_sandbox(parent.revision, installing.clone())
        .await
        .unwrap();
    assert!(store
        .claim_workspace_work(
            &h.workspace.id,
            "workspace-worker",
            now.clone(),
            expires.clone()
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.get_workspace(&h.workspace.id).await.unwrap().unwrap(),
        h.workspace
    );
    let revision = installing.revision;
    installing.credential = fixture.ready_parent.credential.clone();
    installing.revision += 1;
    store.save_sandbox(revision, installing).await.unwrap();
    let claimed = store
        .claim_workspace_work(&h.workspace.id, "workspace-worker", now, expires)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, WorkspaceWorkStep::Prepare);
    assert_eq!(claimed.work.attempts, 0);
    assert_eq!(claimed.revision, h.workspace.revision + 1);
    assert!(claimed.failed.is_none());
    db.drop().await.unwrap();
}
