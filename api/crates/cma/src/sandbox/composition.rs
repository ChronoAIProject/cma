//! Shared production assembly for Sandbox, Workspace, and Agent workers.

use std::sync::Arc;

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
    pub parent_not_ready_pacing_enabled: bool,
    pub bounded_live_fragments: bool,
    pub coordinated_ingress: bool,
    pub access_transport_migration: Option<crate::config::AgentAccessMigration>,
    pub managed_documents_enabled: bool,
    pub event_driven_reconciliation: bool,
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub runtime_version: String,
    pub title_model: String,
}

pub(crate) struct SandboxLifecycleComposition {
    warmup: super::warmup::LegacyWarmSandboxCleanup,
    reconciliation: SandboxReconciler,
    sandbox: SandboxScheduler,
    workspace: SandboxScheduler,
    agent: AgentScheduler,
    capacity_worker: Arc<AgentWorker>,
    capacity_recovery: Option<SandboxScheduler>,
    titles: AgentTitleScheduler,
    suggestions: Arc<super::suggestion_worker::SuggestionWorker>,
    updates: Arc<dyn AgentUpdateRecorder>,
    funding_renewal: FundingRenewalWorker,
}

impl SandboxLifecycleComposition {
    pub(crate) fn new(
        dependencies: SandboxLifecycleDependencies,
        config: SandboxLifecycleConfig,
    ) -> Self {
        let sandbox_admitted = Arc::clone(&dependencies.store.sandbox_admitted);
        let agent_admitted = Arc::clone(&dependencies.store.agent_admitted);
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
        .with_profile_agents(Arc::clone(&dependencies.agents))
        .with_runtime_version(config.runtime_version.clone())
        .with_platform_spend(dependencies.store.clone())
        .with_pending_homes(dependencies.pending_homes.clone())
        .with_failed_runtime_recovery(Arc::new(
            layer2_core::sandbox::SandboxRuntimeUpdateService::new(
                dependencies.store.clone(),
                config.runtime_version.clone(),
            )
            .with_unpublished_recovery(Arc::new(
                super::unpublished_recovery::UnpublishedRuntimeRecovery::new(
                    dependencies.adapter.clone(),
                    config.deployment_id.clone(),
                ),
            )),
        ));
        let updates = DurableAgentUpdateRecorder::with_ingress_budget(
            dependencies.store.clone(),
            dependencies.store.writer_policy.clone(),
            Some(dependencies.store.clone()),
            config.bounded_live_fragments,
            config.coordinated_ingress,
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
            .with_execution(config.execution),
        );
        let agent_worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: Arc::clone(&dependencies.adapter),
                skills: dependencies.skills,
                registry: Arc::clone(&dependencies.registry),
                updates: updates.clone(),
                operations: Arc::clone(&dependencies.operations),
            },
            AgentWorkerConfig::new(
                format!("{}:agent", config.holder),
                config.deployment_id.clone(),
            )
            .with_parent_readiness(Arc::clone(&dependencies.store.agent_parent_ready))
            .with_parent_not_ready_pacing(config.parent_not_ready_pacing_enabled)
            .with_managed_documents(config.managed_documents_enabled)
            .with_access_transport_migration(config.access_transport_migration),
        );
        let funding_renewal = FundingRenewalWorker::new(Arc::clone(&dependencies.funding));
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
            SandboxWorkerConfig::with_policy_source(
                format!("{}:sandbox", config.holder),
                config.deployment_id,
                config.runtime_version,
                dependencies.policy,
            )
            .with_execution(config.execution)
            .with_home_policy(config.home_policy),
            dependencies.funding,
        );

        Self {
            warmup,
            updates,
            reconciliation,
            sandbox: SandboxScheduler::new(sandbox_worker).with_admission_wake(sandbox_admitted),
            workspace: SandboxScheduler::for_workspaces(workspace_worker),
            capacity_worker: agent_worker.clone(),
            capacity_recovery: None,
            agent: AgentScheduler::new(agent_worker, agent_admitted)
                .with_event_driven_reconciliation(
                    Arc::clone(&dependencies.store.agent_parent_ready),
                    config.event_driven_reconciliation,
                ),
            titles,
            funding_renewal,
            suggestions,
        }
    }

    pub(crate) fn updates(&self) -> Arc<dyn AgentUpdateRecorder> {
        Arc::clone(&self.updates)
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

    pub(crate) async fn initialize(&self) -> Result<(), String> {
        self.reconciliation
            .initialize(std::time::SystemTime::now())
            .await
            .map_err(|error| format!("sandbox runtime initialization failed: {error}"))
    }

    pub(crate) fn spawn(
        self,
        shutdown: &mut ShutdownCoordinator,
        workers: &crate::worker_supervision::Workers,
    ) {
        if let Some(scheduler) = self.capacity_recovery {
            shutdown.push(
                scheduler
                    .spawn_supervised(shutdown.subscribe(), workers.get("agent_capacity_recovery")),
            );
        }
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
