//! Shared production composition for Agent-facing public projections.

use std::sync::Arc;

use axum::Router;
use layer1_public::agui::AguiState;
use layer1_public::responses::ResponsesState;
use layer1_public::rest::{agent_router, AgentRoutesState};
use layer2_core::profiles::{AgentProfileStore, ProfileUseRecorder};
use layer2_core::sandbox::store::{AgentContextStore, AgentReadStore, AgentServiceStore};
use layer2_core::sandbox::{
    AgentContextService, AgentOperationService, AgentOperationStore, AgentReadService,
    AgentRuntimeRegistry, AgentService, Sandbox, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading, SandboxService, WorkspaceService,
};
use layer2_core::{OperationStore, PlatformSettingsService, RuntimeCapabilities};

pub(crate) struct AgentProjectionServices {
    agents: Arc<AgentService>,
    context: Arc<AgentContextService>,
    operations: Arc<AgentOperationService>,
    reads: Arc<AgentReadService>,
    sandboxes: Arc<SandboxService>,
}

impl AgentProjectionServices {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new<S, P, R, Q>(
        store: Arc<S>,
        profiles: Arc<P>,
        settings: Arc<PlatformSettingsService>,
        sandboxes: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        runtimes: Arc<R>,
        resources: Arc<Q>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Self
    where
        S: AgentServiceStore + AgentContextStore + AgentReadStore + AgentOperationStore + 'static,
        P: AgentProfileStore + ProfileUseRecorder + 'static,
        R: AgentRuntimeRegistry + 'static,
        Q: SandboxResourceReader + 'static,
    {
        let agents = AgentService::new(
            store.clone(),
            profiles.clone(),
            runtimes.clone(),
            runtime_capabilities,
        );
        let context = AgentContextService::new(
            store.clone(),
            profiles,
            settings,
            sandboxes.clone(),
            workspaces,
            agents.clone(),
        );
        let reads = AgentReadService::new(store.clone(), resources);
        let operations = AgentOperationService::new(store, runtimes);
        Self {
            agents,
            context,
            operations,
            reads,
            sandboxes,
        }
    }

    pub(crate) fn router(self, operation_records: Arc<dyn OperationStore>) -> Router {
        let agent_routes = AgentRoutesState::new(
            self.agents,
            self.reads.clone(),
            self.operations.clone(),
            operation_records,
        );
        let responses = ResponsesState::new(
            self.context,
            self.operations.clone(),
            self.reads.clone(),
            self.sandboxes,
        );
        let agui = AguiState::new(self.operations, self.reads);
        Router::new()
            .merge(agent_router(agent_routes))
            .merge(layer1_public::responses::router(responses))
            .merge(layer1_public::agui::router(agui))
            .merge(layer1_public::pty::router())
            .merge(layer1_public::mcp::router())
            .merge(layer1_public::a2a::router())
    }
}

#[derive(Default)]
pub(crate) struct UnavailableSandboxResources;

#[async_trait::async_trait]
impl SandboxResourceReader for UnavailableSandboxResources {
    async fn sandbox_resources(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        Err(SandboxResourceReadError::Unavailable(
            "the hosted Sandbox resource transport is not composed".into(),
        ))
    }
}
