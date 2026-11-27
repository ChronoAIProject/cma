//! Shared production assembly for Sandbox, Workspace, and Agent workers.

use std::{sync::Arc, time::Duration};

use layer2_core::sandbox::{
    AgentOperationService, AgentService, AgentTitleService, AgentUpdateRecorder,
    DurableAgentUpdateRecorder, SandboxPolicySource, WorkspaceService,
};
use layer3_adapter::SandboxAdapter;

use super::{
    agent_scheduler::AgentScheduler,
    agent_skills::AgentSkillResolver,
    agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies},
    credential::SandboxCredentialController,
    funding::{FundingRenewalWorker, SandboxFundingAdapter},
    live_agent_stopper::DurableLiveAgentStopper,
    reconciliation::SandboxReconciler,
    scheduler::SandboxScheduler,
    title_scheduler::AgentTitleScheduler,
    title_worker::{AgentTitleWorker, TitleRuntimeEndpoint},
    worker::{SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies},
    workspace_worker::{
        DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
        WorkspaceWorkerDependencies,
    },
    LiveAgentRegistry,
};
use crate::{shutdown::ShutdownCoordinator, store::MongoSandboxStore};

pub(crate) struct SandboxLifecycleDependencies {
    pub store: Arc<MongoSandboxStore>,
    pub pending_homes: Arc<crate::store::pending_launch_homes::PendingLaunchHomes>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub credentials: Arc<dyn SandboxCredentialController>,
    pub registry: Arc<LiveAgentRegistry>,
    pub workspaces: Arc<WorkspaceService>,
    pub agents: Arc<AgentService>,
    pub operations: Arc<AgentOperationService>,
    pub policy: Arc<dyn SandboxPolicySource>,
    pub skills: Arc<dyn AgentSkillResolver>,
    pub title_runtime: Arc<dyn TitleRuntimeEndpoint>,
    pub title_budget: Arc<dyn super::title_budget::TitleBudget>,
    pub funding: Arc<dyn SandboxFundingAdapter>,
}

pub(crate) struct SandboxLifecycleConfig {
    pub renewable_work: bool,
    pub workspace_prepare_observations: bool,
    pub ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    pub runtime_loss_bounded_proof_enabled: bool,
    pub native_retirement_finality_enabled: bool,
    pub native_retirement_capture_enabled: bool,
    pub parent_not_ready_pacing_enabled: bool,
    pub inspection_refusal: Option<crate::config::InspectionRefusalControl>,
    pub startup_sibling_hold_enabled: bool,
    pub retry_single_marker_enabled: bool,
    pub work_retry_policy_enabled: bool,
    pub native_create_retry_enabled: bool,
    pub reopen_output_refusal_enabled: bool,
    pub park_runtime_update_sandbox: Option<layer2_core::SandboxId>,
    pub cleanup_continuation_enabled: bool,
    pub owner_stop_settlement_enabled: bool,
    pub bounded_live_fragments: bool,
    pub quiet_output: Option<layer2_core::sandbox::AgentOutputCoalescePolicy>,
    pub coordinated_ingress: bool,
    pub access_transport_migration: Option<crate::config::AgentAccessMigration>,
    pub managed_documents_enabled: bool,
    pub event_driven_reconciliation: bool,
    pub resume_agent_reconciliation: bool,
    pub stale_turn_reconciliation: bool,
    pub stale_turn_terminal: bool,
    pub skip_ineligible_reconciliation: bool,
    pub defer_foreign_reconciliation: bool,
    pub sandbox_reconciliation_reserved_lane: bool,
    pub sandbox_binding_hold_metrics: bool,
    pub startup_native_home_hold_enabled: bool,
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    pub title_model: String,
}

pub(crate) struct SandboxLifecycleComposition {
    inline_lifecycle: Option<crate::agent_lifecycle::LifecycleDrain>,
    pub(crate) capture_drain: Option<crate::agent_lifecycle::LifecycleDrain>,
    pub(crate) reopen_worker: Arc<SandboxWorker>,
    warmup: super::warmup::LegacyWarmSandboxCleanup,
    reconciliation: SandboxReconciler,
    sandbox: SandboxScheduler,
    workspace: SandboxScheduler,
    agent: AgentScheduler,
    capacity_worker: Arc<AgentWorker>,
    capacity_recovery: Option<SandboxScheduler>,
    stale_turns: Option<super::stale_turn_reconciliation::Worker>,
    turn_timeouts: super::turn_timeout_worker::Worker,
    titles: AgentTitleScheduler,
    suggestions: Arc<super::suggestion_worker::SuggestionWorker>,
    updates: Arc<DurableAgentUpdateRecorder>,
    funding_renewal: FundingRenewalWorker,
}

impl SandboxLifecycleComposition {
    pub(crate) fn binding_diagnostics(&self) -> Arc<dyn crate::binding_diagnostics::LocalSource> {
        self.reconciliation.binding_diagnostics()
    }

    pub(crate) fn register_inline_lifecycle(
        &mut self,
        drain: crate::agent_lifecycle::LifecycleDrain,
    ) {
        assert!(
            self.inline_lifecycle.is_none(),
            "lifecycle drain registered once"
        );
        self.inline_lifecycle = Some(drain);
    }

    pub(crate) fn new(
        dependencies: SandboxLifecycleDependencies,
        config: SandboxLifecycleConfig,
    ) -> Result<Self, String> {
        let shared = crate::background::shared();
        let reconciliation_budget = if config.sandbox_reconciliation_reserved_lane {
            shared
                .reserve(1)
                .ok_or("cannot reserve Sandbox reconciliation capacity before worker startup")?
        } else {
            shared
        };
        tracing::info!(
            reserved_lane = config.sandbox_reconciliation_reserved_lane,
            "sandbox reconciliation background scheduling configured"
        );
        let sandbox_admitted = Arc::clone(&dependencies.store.sandbox_admitted);
        let agent_admitted = Arc::clone(&dependencies.store.agent_admitted);
        let turn_timeouts = super::turn_timeout_worker::Worker::new(
            dependencies.store.clone(),
            config.ownership.clone(),
        );
        let stale_turns = if config.stale_turn_reconciliation {
            config.ownership.as_ref().map(|ownership| {
                super::stale_turn_reconciliation::Worker::new(
                    dependencies.store.clone(),
                    ownership.clone(),
                    dependencies.registry.clone(),
                    config
                        .stale_turn_terminal
                        .then(|| dependencies.adapter.clone()),
                )
            })
        } else {
            None
        };
        let warmup = super::warmup::LegacyWarmSandboxCleanup {
            store: dependencies.store.clone(),
            adapter: dependencies.adapter.clone(),
        };
        let reconciliation = SandboxReconciler::new(
            dependencies.store.clone(),
            Arc::clone(&dependencies.adapter),
            Arc::clone(&dependencies.policy),
            config.deployment_id.clone(),
        )
        .with_background_work(crate::background::BackgroundWork::new(
            reconciliation_budget,
            "sandbox_reconciliation",
            Duration::from_secs(30),
            Duration::from_secs(15),
        ))
        .with_background_resumption(config.sandbox_reconciliation_reserved_lane)
        .with_binding_hold_metrics(config.sandbox_binding_hold_metrics)
        .with_startup_native_home_hold(config.startup_native_home_hold_enabled)
        .with_runtime_loss_bounded_proof(config.runtime_loss_bounded_proof_enabled)
        .with_profile_agents(Arc::clone(&dependencies.agents))
        .with_released_runtime(config.released.clone())
        .with_platform_spend(dependencies.store.clone())
        .with_failed_runtime_recovery(Arc::new(
            layer2_core::sandbox::SandboxRuntimeUpdateService::with_released(
                dependencies.store.clone(),
                config.released.clone(),
            )
            .with_unpublished_recovery(Arc::new(
                super::unpublished_recovery::UnpublishedRuntimeRecovery::new(
                    dependencies.adapter.clone(),
                    config.deployment_id.clone(),
                ),
            )),
        ));
        let reconciliation = if dependencies.pending_homes.enabled() {
            reconciliation.with_pending_homes(dependencies.pending_homes.clone())
        } else {
            reconciliation
        };
        let updates = DurableAgentUpdateRecorder::with_output_policy(
            dependencies.store.clone(),
            dependencies.store.writer_policy.clone(),
            Some(dependencies.store.clone()),
            config.bounded_live_fragments,
            config.coordinated_ingress,
            config.quiet_output,
        );
        let registry = Arc::clone(&dependencies.registry);
        let suggestions = super::suggestion_worker::SuggestionWorker::new(
            dependencies.store.clone(),
            dependencies.credentials.clone(),
            dependencies.title_runtime.clone(),
            config.title_model.clone(),
        );
        let titles = AgentTitleScheduler::new(
            AgentTitleWorker::new(
                AgentTitleService::new(
                    dependencies.store.clone(),
                    format!("{}:title", config.holder),
                )
                .expect("the deployment worker holder is bounded and nonempty"),
                dependencies.store.clone(),
                Arc::clone(&dependencies.credentials),
                dependencies.title_runtime,
                config.title_model,
                dependencies.title_budget,
            )
            .with_request_budget(dependencies.store.clone()),
        );
        let workspace_worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: Arc::clone(&dependencies.adapter),
            },
            WorkspaceWorkerConfig::new(
                format!("{}:workspace", config.holder),
                config.deployment_id.clone(),
            )
            .with_execution(config.execution)
            .with_replicas(config.renewable_work)
            .with_prepare_observations(config.workspace_prepare_observations),
        );
        let capture_delivery = (config.native_retirement_finality_enabled
            && config.native_retirement_capture_enabled
            && config.ownership.is_some())
        .then(|| {
            super::native_retirement_delivery::CaptureDelivery::new(
                dependencies.store.clone(),
                crate::store::DISCOVERY_BATCH,
            )
        });
        let capture_drain = capture_delivery
            .as_ref()
            .map(|delivery| delivery.tasks.clone());
        let mut agent_config = AgentWorkerConfig::new(
            format!("{}:agent", config.holder),
            config.deployment_id.clone(),
        )
        .with_parent_readiness(Arc::clone(&dependencies.store.agent_parent_ready))
        .with_ownership(config.ownership)
        .with_skip_ineligible_reconciliation(config.skip_ineligible_reconciliation)
        .with_defer_foreign_reconciliation(config.defer_foreign_reconciliation)
        .with_ownership_retry_hints(dependencies.store.ownership_retry_hints())
        .with_parent_not_ready_pacing(config.parent_not_ready_pacing_enabled)
        .with_startup_sibling_hold(config.startup_sibling_hold_enabled)
        .with_cleanup_continuation(config.cleanup_continuation_enabled)
        .with_retry_single_marker(config.retry_single_marker_enabled)
        .with_work_retry_policy(config.work_retry_policy_enabled)
        .with_managed_documents(config.managed_documents_enabled)
        .with_access_transport_migration(config.access_transport_migration);
        agent_config.owner_stop_settlement_enabled = config.owner_stop_settlement_enabled;
        agent_config.inspection_refusal = config.inspection_refusal;
        if config.native_retirement_finality_enabled {
            agent_config.retirement_reporter = Some(dependencies.store.clone());
        }
        agent_config.capture_delivery = capture_delivery;
        let agent_worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: Arc::clone(&dependencies.adapter),
                skills: dependencies.skills,
                registry: Arc::clone(&dependencies.registry),
                updates: updates.clone(),
                operations: Arc::clone(&dependencies.operations),
            },
            agent_config,
        );
        let funding_renewal = FundingRenewalWorker::new(Arc::clone(&dependencies.funding));
        let mut retirement_finalizer =
            super::native_retirement_finalizer::NativeRetirementFinalizer::new(
                dependencies.store.clone(),
                dependencies.adapter.clone(),
                config.deployment_id.clone(),
                dependencies.store.clone(),
                config.native_retirement_finality_enabled,
            );
        if config.native_retirement_finality_enabled {
            retirement_finalizer = retirement_finalizer
                .with_capture(dependencies.store.clone(), capture_drain.clone());
        }
        let retirement_finalizer = Arc::new(retirement_finalizer);
        let mut sandbox_config = SandboxWorkerConfig::with_policy_source(
            format!("{}:sandbox", config.holder),
            config.deployment_id,
            config.released.snapshot().agent,
            dependencies.policy,
        )
        .with_released_runtime(config.released.clone())
        .with_execution(config.execution)
        .with_replicas(config.renewable_work)
        .with_work_retry_policy(config.work_retry_policy_enabled)
        .with_native_create_retry(config.native_create_retry_enabled)
        .with_home_policy(config.home_policy);
        sandbox_config.retirement_finalizer = Some(retirement_finalizer);
        sandbox_config.reopen_output_refusal_enabled = config.reopen_output_refusal_enabled;
        sandbox_config.park_runtime_update_sandbox = config.park_runtime_update_sandbox;
        let sandbox_worker = SandboxWorker::new_with_funding(
            SandboxWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: dependencies.adapter,
                credentials: dependencies.credentials,
                workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                    dependencies.workspaces,
                    Arc::clone(&workspace_worker),
                )),
                agents: Arc::new(DurableLiveAgentStopper::new(
                    dependencies.store.clone(),
                    Arc::clone(&registry),
                )),
                registry,
                updates: updates.clone(),
                operations: dependencies.operations,
            },
            sandbox_config,
            dependencies.funding,
        );

        Ok(Self {
            inline_lifecycle: None,
            capture_drain,
            reopen_worker: sandbox_worker.clone(),
            warmup,
            updates,
            reconciliation,
            sandbox: SandboxScheduler::new(sandbox_worker).with_admission_wake(sandbox_admitted),
            workspace: SandboxScheduler::for_workspaces(workspace_worker),
            capacity_worker: agent_worker.clone(),
            capacity_recovery: None,
            stale_turns,
            turn_timeouts,
            agent: AgentScheduler::new(agent_worker, agent_admitted)
                .with_resumable_reconciliation(config.resume_agent_reconciliation)
                .with_event_driven_reconciliation(
                    Arc::clone(&dependencies.store.agent_parent_ready),
                    config.event_driven_reconciliation,
                ),
            titles,
            funding_renewal,
            suggestions,
        })
    }

    pub(crate) fn live_updates(&self) -> Arc<DurableAgentUpdateRecorder> {
        self.updates.clone()
    }

    pub(crate) fn updates(&self) -> Arc<dyn AgentUpdateRecorder> {
        self.updates.clone()
    }

    pub(crate) fn capacity_recovery_commands(
        &mut self,
        store: Arc<MongoSandboxStore>,
        service: Arc<crate::store::agent_capacity_recovery::RecoveryService>,
    ) -> Arc<dyn layer2_core::sandbox::agent_capacity_recovery::commands::CapacityRecoveryCommands>
    {
        assert!(
            self.capacity_recovery.is_none(),
            "capacity recovery is assembled once"
        );
        self.capacity_recovery = Some(SandboxScheduler::for_capacity_recovery(
            super::capacity_scheduler::CapacityRecoverySweep::new(
                store,
                service.clone(),
                self.capacity_worker.clone(),
            ),
        ));
        self.capacity_worker
            .clone()
            .capacity_recovery_commands(service)
    }

    pub(crate) async fn initialize(&self, stopping: impl Fn() -> bool) -> Result<(), String> {
        self.reconciliation
            .initialize_before_workers_until(std::time::SystemTime::now(), stopping)
            .await
            .map_err(|error| format!("sandbox runtime initialization failed: {error}"))
    }

    pub(crate) fn spawn(
        self,
        shutdown: &mut ShutdownCoordinator,
        workers: &crate::worker_supervision::Workers,
    ) {
        if let Some(lifecycle) = self.inline_lifecycle {
            shutdown.register_lifecycle(lifecycle);
        }
        if let Some(scheduler) = self.capacity_recovery {
            shutdown.push(
                scheduler
                    .spawn_supervised(shutdown.subscribe(), workers.get("agent_capacity_recovery")),
            );
        }
        if let Some(worker) = self.stale_turns {
            shutdown.push(worker.spawn(
                shutdown.subscribe(),
                workers.get("agent_stale_turn_reconciliation"),
            ));
        }
        shutdown.push(
            self.turn_timeouts
                .spawn(shutdown.subscribe(), workers.get("agent_turn_timeout")),
        );
        shutdown.push(self.warmup.spawn(shutdown.subscribe()));
        let signal = shutdown.subscribe();
        shutdown.push(
            self.reconciliation
                .spawn_supervised(signal, workers.get("sandbox_reconciliation")),
        );
        shutdown.push(
            self.agent
                .spawn_supervised(shutdown.subscribe(), workers.get("agents")),
        );
        shutdown.push(self.titles.spawn(shutdown.subscribe()));
        shutdown.push(self.funding_renewal.spawn(shutdown.subscribe()));
        shutdown.push(self.suggestions.spawn(shutdown.subscribe()));
        for (scheduler, name) in [(self.sandbox, "sandboxes"), (self.workspace, "workspaces")] {
            let signal = shutdown.subscribe();
            shutdown.push(scheduler.spawn_supervised(signal, workers.get(name)));
        }
    }
}
