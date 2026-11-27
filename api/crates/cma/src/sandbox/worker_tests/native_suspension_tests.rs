use super::*;
use crate::sandbox::reconciliation::SandboxReconciler;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore};
use layer2_core::sandbox::{AgentStatus, SandboxLifecycleOperation, SandboxRevivePhase};
mod backend;
mod binding_retirement_tests;
mod custom_runtime_tests;
mod nonexecuting_recovery_tests;
mod recovery_request_tests;
mod recovery_tests;
mod reopen_retirement_tests;
mod reserved_recovery_tests;
mod retained_reopen_tests;
mod retire_action_matrix_tests;
mod retirement_case6;
pub(crate) use retirement_case6::exercise_native_retirement_finality;
mod retirement_credentials_tests;
mod retirement_retry_tests;
mod retirement_tests;
mod reviewer_boundary_retained;
mod runtime_update_tests;
pub(crate) use runtime_update_tests::exercise_runtime_update_reopen_http;
mod scheduling_tests;
mod service_access;
mod status_reuse_tests;
use backend::NativeBackend;

struct Fixture {
    harness: Harness,
    homes: Arc<home_tests::Homes>,
    backend: NativeBackend,
    /// Native retirement admission; the production default is off.
    retirement_admission: std::sync::atomic::AtomicBool,
    reopen_output_refusal: bool,
    park_runtime_update_sandbox: Option<SandboxId>,
    retry_delay: Duration,
}
impl Fixture {
    async fn new() -> Self {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let homes = home_tests::Homes::new(&harness);
        let backend = NativeBackend::new(harness.backend.clone());
        backend
            .retirement_records
            .store
            .set(harness.store.clone())
            .ok()
            .unwrap();
        Self {
            harness,
            homes,
            backend,
            retirement_admission: std::sync::atomic::AtomicBool::new(true),
            reopen_output_refusal: false,
            park_runtime_update_sandbox: None,
            retry_delay: Duration::from_secs(5),
        }
    }
    /// Exercise the production default, where finality admission is off.
    fn without_retirement_admission(self) -> Self {
        self.retirement_admission.store(false, Ordering::SeqCst);
        self
    }
    async fn native() -> Self {
        let mut fixture = Self::new().await;
        fixture.backend.pools = Some(super::native_pool_tests::pools(&fixture.harness, &[]));
        fixture
    }
    fn home_policy(&self) -> layer3_adapter::HomeVolumePolicy {
        if self.backend.pools.is_some() {
            layer3_adapter::HomeVolumePolicy {
                mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: String::new(),
                template_generation: String::new(),
            }
        } else {
            home_tests::home_policy()
        }
    }
    fn adapter(&self, image: &str) -> Arc<dyn SandboxAdapter> {
        let mut config = CodexSandboxSpec::default();
        config.deployment.image.uri = image.into();
        let homes: Arc<dyn layer3_adapter::HomeVolumeStore> = match &self.backend.pools {
            Some(pool) => pool.clone(),
            None => self.homes.clone(),
        };
        *self.backend.retirement_home.lock().unwrap() = Some(homes.clone());
        Arc::new(CodexAdapter::with_backend(self.backend.clone(), config).with_home_volumes(homes))
    }
    async fn step(&self, sandbox: &SandboxId, now: SystemTime, image: &str) {
        let adapter = self.adapter(image);
        SandboxReconciler::new(
            self.harness.store.clone(),
            adapter.clone(),
            Arc::new(self.harness.policy.clone()),
            "test".into(),
        )
        .initialize(now)
        .await
        .unwrap();
        let mut config =
            SandboxWorkerConfig::new("restarted", "test", image, self.harness.policy.clone())
                .with_home_policy(Some(self.home_policy()));
        config.retry_delay = self.retry_delay;
        config.reopen_output_refusal_enabled = self.reopen_output_refusal;
        config.park_runtime_update_sandbox = self.park_runtime_update_sandbox.clone();
        config.retirement_finalizer = Some(Arc::new(
            crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer::new(
                self.harness.store.clone(),
                adapter.clone(),
                "test".into(),
                self.backend.retirement_records.clone(),
                self.retirement_admission.load(Ordering::SeqCst),
            )
            .with_clock(Arc::new(move || now + Duration::from_secs(1))),
        ));
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: self.harness.store.clone(),
                adapter: adapter.clone(),
                credentials: self.harness.credentials.clone(),
                workspaces: Arc::new(WorkspaceRecorder {
                    actions: self.harness.actions.clone(),
                }),
                agents: Arc::new(
                    crate::sandbox::live_agent_stopper::DurableLiveAgentStopper::new(
                        self.harness.store.clone(),
                        self.harness.registry.clone(),
                    ),
                ),
                registry: self.harness.registry.clone(),
                updates: DurableAgentUpdateRecorder::new(self.harness.store.clone()),
                operations: AgentOperationService::new(
                    self.harness.store.clone(),
                    self.harness.registry.clone(),
                ),
            },
            config,
        );
        let progressed = worker.run_one(sandbox, now).await.unwrap();
        let stored = self.harness.stored(sandbox).await;
        assert!(
            progressed || stamp_time(&stored.work.ready_at) > now,
            "worker did not claim work at {now:?}: status={:?}, work={:?}, failure={:?}",
            stored.status,
            stored.work,
            stored.failure
        );
        if stored.status == SandboxStatus::Retiring {
            use crate::sandbox::agent_worker::{
                AgentWorker, AgentWorkerConfig, AgentWorkerDependencies,
            };
            let mut config = AgentWorkerConfig::new("retiring-agent", "test");
            config.retry_delay = self.retry_delay;
            let worker = AgentWorker::new(
                AgentWorkerDependencies {
                    store: self.harness.store.clone(),
                    adapter,
                    skills: Arc::new(crate::sandbox::agent_skills::EmptyAgentSkillResolver),
                    registry: self.harness.registry.clone(),
                    updates: DurableAgentUpdateRecorder::new(self.harness.store.clone()),
                    operations: AgentOperationService::new(
                        self.harness.store.clone(),
                        self.harness.registry.clone(),
                    ),
                },
                config,
            );
            for id in worker.due_work(now, 100).await.unwrap() {
                worker.run_one(&id, now).await.unwrap();
            }
        }
    }
    async fn drive(
        &self,
        id: &SandboxId,
        now: &mut SystemTime,
        status: SandboxStatus,
        image: &str,
    ) -> Sandbox {
        for _ in 0..20 {
            let sandbox = self.harness.stored(id).await;
            if sandbox.status == status {
                return sandbox;
            }
            assert_ne!(
                sandbox.status,
                SandboxStatus::Failed,
                "{:?}",
                sandbox.failure
            );
            *now += Duration::from_secs(6);
            *now = (*now).max(stamp_time(&sandbox.work.ready_at));
            self.step(id, *now, image).await;
        }
        let stored = self.harness.stored(id).await;
        panic!(
            "lifecycle did not reach {status:?}: status={:?}, work={:?}, failure={:?}",
            stored.status, stored.work, stored.failure
        );
    }
    async fn pause(&self, active: &Sandbox, now: &mut SystemTime) -> Sandbox {
        self.harness
            .store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                active.id.clone(),
                active.owner.clone(),
                rfc3339_at(*now),
                None,
            ))
            .await
            .unwrap();
        self.drive(&active.id, now, SandboxStatus::Paused, &runtime_image())
            .await
    }
}

#[tokio::test]
async fn native_pause_resume_survives_lost_responses_and_each_control_plane_restart() {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let first = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    let second = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 2).await;
    let original = fixture.backend.snapshot().await;
    fixture.backend.lose_suspend.store(true, Ordering::SeqCst);
    let paused = fixture.pause(&active, &mut now).await;
    assert!(paused.runtime.is_none());
    assert_eq!(
        paused.lifecycle.suspended_runtime.as_deref(),
        active.runtime.as_ref()
    );
    assert_eq!(paused.lifecycle.home, active.lifecycle.home);
    assert!(paused.lifecycle.checkpoints.latest.is_none());
    fixture.backend.lose_resume.store(true, Ordering::SeqCst);
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            rfc3339_at(now),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    let resumed = fixture
        .drive(
            &paused.id,
            &mut now,
            SandboxStatus::Active,
            "cma-runtime:changed-default",
        )
        .await;
    let previous = active.runtime.unwrap();
    let current = resumed.runtime.unwrap();
    assert_eq!(current.runtime_id, previous.runtime_id);
    assert_eq!(current.runtime_version, previous.runtime_version);
    assert!(current.provision_generation > previous.provision_generation);
    assert_ne!(current.boot_id, previous.boot_id);
    assert!(resumed.lifecycle.suspended_runtime.is_none());
    assert_eq!(resumed.lifecycle.home, paused.lifecycle.home);
    let restored = fixture.backend.snapshot().await;
    assert_eq!(restored.workspaces, original.workspaces);
    assert_eq!(restored.worktrees, original.worktrees);
    assert_eq!(restored.homes, original.homes);
    for before in [first, second] {
        let after = fixture
            .harness
            .store
            .get_agent(&before.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, AgentStatus::Ready);
        assert_eq!(
            after.conversation.conversation_id,
            before.conversation.conversation_id
        );
        assert!(after.conversation.runtime_generation > before.conversation.runtime_generation);
    }
    let calls = fixture.backend.calls.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|action| action.as_str() == "create")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|action| action.as_str() == "suspend")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|action| action.as_str() == "resume")
            .count(),
        1
    );
    assert!(calls.contains(&"resume-replay".into()));
    assert!(!calls.contains(&"delete-retained".into()));
}

#[tokio::test]
async fn deleting_a_paused_native_sandbox_removes_its_crd_before_releasing_the_home() {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    fixture
        .harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    let paused = fixture.pause(&active, &mut now).await;
    let mut retiring = paused.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    fixture
        .harness
        .store
        .save_sandbox(paused.revision, retiring)
        .await
        .unwrap();
    let retired = fixture
        .drive(
            &paused.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.runtime.is_none());
    assert!(retired.lifecycle.suspended_runtime.is_none());
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    let stopped = fixture
        .harness
        .store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(
        stopped.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(stopped.work.receipts, agent.work.receipts);
    assert!(fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
}

#[tokio::test]
async fn retirement_after_lost_suspend_response_deletes_the_stopped_crd_and_settles_agents() {
    let mut fixture = Fixture::new().await;
    let created = fixture.harness.create().await;
    let mut now = start_time(&created);
    let active = fixture
        .drive(
            &created.id,
            &mut now,
            SandboxStatus::Active,
            &runtime_image(),
        )
        .await;
    fixture.harness.adapter = fixture.adapter(&runtime_image());
    crate::sandbox::runtime_attachment::adopt_bound_runtime(
        fixture.harness.adapter.as_ref(),
        &active,
        "test",
    )
    .await
    .unwrap();
    let agent = lifecycle_recovery_tests::prepare_agent(&fixture.harness, &active, now, 1).await;
    fixture
        .harness
        .store
        .create_workspace(lifecycle_recovery_tests::workspace_for(&agent, now))
        .await
        .unwrap();
    fixture.backend.lose_suspend.store(true, Ordering::SeqCst);
    fixture
        .harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    for _ in 0..4 {
        if fixture.backend.calls.snapshot().contains(&"suspend".into()) {
            break;
        }
        now += Duration::from_secs(6);
        fixture.step(&active.id, now, &runtime_image()).await;
    }
    assert!(fixture.backend.calls.snapshot().contains(&"suspend".into()));
    let pending = fixture.harness.stored(&active.id).await;
    assert_eq!(pending.runtime, active.runtime);
    assert!(pending.lifecycle.suspended_runtime.is_none());
    fixture.backend.lose_delete.store(true, Ordering::SeqCst);
    let mut retiring = pending.clone();
    retiring.begin_retirement(rfc3339_at(now)).unwrap();
    fixture
        .harness
        .store
        .save_sandbox(pending.revision, retiring)
        .await
        .unwrap();
    let retired = fixture
        .drive(
            &active.id,
            &mut now,
            SandboxStatus::Retired,
            &runtime_image(),
        )
        .await;
    assert!(retired.lifecycle.home.as_ref().unwrap().released);
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Stopped
    );
    assert!(fixture
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
}
