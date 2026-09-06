//! Shared production assembly for Sandbox, Workspace, and Agent workers.

use std::sync::Arc;

use layer2_core::sandbox::{
    AgentOperationService, DurableAgentUpdateRecorder, SandboxPolicySource, WorkspaceService,
};
use layer3_adapter::SandboxAdapter;

use super::{
    agent_skills::AgentSkillResolver,
    agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies},
    checkpoint_maintenance::SandboxCheckpointMaintenance,
    credential::SandboxCredentialController,
    live_agent_stopper::DurableLiveAgentStopper,
    reconciliation::SandboxReconciler,
    scheduler::SandboxScheduler,
    worker::{
        CheckpointObjectPurger, SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies,
    },
    workspace_worker::{
        DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
        WorkspaceWorkerDependencies,
    },
    LiveAgentRegistry,
};
use crate::{shutdown::ShutdownCoordinator, store::MongoSandboxStore};

pub(crate) struct SandboxLifecycleDependencies {
    pub store: Arc<MongoSandboxStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub credentials: Arc<dyn SandboxCredentialController>,
    pub checkpoints: Arc<dyn CheckpointObjectPurger>,
    pub registry: Arc<LiveAgentRegistry>,
    pub workspaces: Arc<WorkspaceService>,
    pub operations: Arc<AgentOperationService>,
    pub policy: Arc<dyn SandboxPolicySource>,
    pub skills: Arc<dyn AgentSkillResolver>,
}

pub(crate) struct SandboxLifecycleConfig {
    pub holder: String,
    pub deployment_id: String,
    pub runtime_version: String,
}

pub(crate) struct SandboxLifecycleComposition {
    reconciliation: SandboxReconciler,
    sandbox: SandboxScheduler,
    workspace: SandboxScheduler,
    agent: SandboxScheduler,
    checkpoints: SandboxScheduler,
}

impl SandboxLifecycleComposition {
    pub(crate) fn new(
        dependencies: SandboxLifecycleDependencies,
        config: SandboxLifecycleConfig,
    ) -> Self {
        let reconciliation = SandboxReconciler::new(
            dependencies.store.clone(),
            Arc::clone(&dependencies.adapter),
            Arc::clone(&dependencies.policy),
            config.deployment_id.clone(),
            config.runtime_version.clone(),
        );
        let checkpoints = SandboxScheduler::for_checkpoints(SandboxCheckpointMaintenance::new(
            dependencies.store.clone(),
            Arc::clone(&dependencies.adapter),
            config.deployment_id.clone(),
            format!("{}:checkpoint", config.holder),
        ));
        let updates = DurableAgentUpdateRecorder::new(Arc::clone(&dependencies.store));
        let registry = Arc::clone(&dependencies.registry);
        let workspace_worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: Arc::clone(&dependencies.adapter),
            },
            WorkspaceWorkerConfig::new(
                format!("{}:workspace", config.holder),
                config.deployment_id.clone(),
            ),
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
            ),
        );
        let sandbox_worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: dependencies.store.clone(),
                adapter: dependencies.adapter,
                credentials: dependencies.credentials,
                workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                    dependencies.workspaces,
                    Arc::clone(&workspace_worker),
                )),
                agents: Arc::new(DurableLiveAgentStopper::new(
                    dependencies.store,
                    Arc::clone(&registry),
                )),
                checkpoints: dependencies.checkpoints,
                registry,
                updates,
                operations: dependencies.operations,
            },
            SandboxWorkerConfig::with_policy_source(
                format!("{}:sandbox", config.holder),
                config.deployment_id,
                config.runtime_version,
                dependencies.policy,
            ),
        );

        Self {
            reconciliation,
            checkpoints,
            sandbox: SandboxScheduler::new(sandbox_worker),
            workspace: SandboxScheduler::for_workspaces(workspace_worker),
            agent: SandboxScheduler::for_agents(agent_worker),
        }
    }

    pub(crate) async fn initialize(&self) -> Result<(), String> {
        self.reconciliation
            .initialize(std::time::SystemTime::now())
            .await
            .map_err(|error| format!("sandbox runtime initialization failed: {error}"))
    }

    pub(crate) fn spawn(self, shutdown: &mut ShutdownCoordinator) {
        let signal = shutdown.subscribe();
        shutdown.push(self.reconciliation.spawn(signal));
        for scheduler in [self.checkpoints, self.sandbox, self.workspace, self.agent] {
            let signal = shutdown.subscribe();
            shutdown.push(scheduler.spawn(signal));
        }
    }
}
