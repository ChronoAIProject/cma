//! Shared production composition for Agent-facing public projections.

use std::sync::Arc;

use axum::Router;
use layer1_public::rest::{agent_router, AgentRoutesState};
use layer2_core::profiles::{AgentProfileStore, ProfileUseRecorder};
use layer2_core::sandbox::store::{AgentReadStore, AgentServiceStore};
use layer2_core::sandbox::{
    AgentOperationService, AgentOperationStore, AgentReadService, AgentRuntimeRegistry,
    AgentService, Sandbox, SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
};
use layer2_core::{OperationStore, RuntimeCapabilities};

pub(crate) struct AgentProjectionServices {
    agents: Arc<AgentService>,
    operations: Arc<AgentOperationService>,
    reads: Arc<AgentReadService>,
}

impl AgentProjectionServices {
    pub(crate) fn new<S, P, R, Q>(
        store: Arc<S>,
        profiles: Arc<P>,
        runtimes: Arc<R>,
        resources: Arc<Q>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Self
    where
        S: AgentServiceStore + AgentReadStore + AgentOperationStore + 'static,
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
        let reads = AgentReadService::new(store.clone(), resources);
        let operations = AgentOperationService::new(store, runtimes);
        Self {
            agents,
            operations,
            reads,
        }
    }

    pub(crate) fn agents(&self) -> Arc<AgentService> {
        Arc::clone(&self.agents)
    }

    pub(crate) fn router(&self, operation_records: Arc<dyn OperationStore>) -> Router {
        let agent_routes = AgentRoutesState::new(
            Arc::clone(&self.agents),
            Arc::clone(&self.reads),
            Arc::clone(&self.operations),
            operation_records,
        );
        Router::new().merge(agent_router(agent_routes))
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
