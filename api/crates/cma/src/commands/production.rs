//! Retained command execution remains composed independently of fresh ingress.
use std::sync::Arc;

use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::{
        agent_io::{AgentCommandAuthorizer, AgentCommandWorkerStore},
        AgentContextService, AgentOperationService,
    },
};

use super::ResourceCommandExecution;

pub(crate) struct ProductionCommandDependencies {
    pub source_guard: Option<Arc<dyn layer2_core::sandbox::agent_io::CommandSourceGuard>>,
    pub authority: Arc<dyn AgentCommandAuthorizer>,
    pub access: Arc<ResourceAccessService>,
    pub store: Arc<dyn AgentCommandWorkerStore>,
    pub operations: Arc<AgentOperationService>,
    pub context: Arc<AgentContextService>,
}

impl ResourceCommandExecution {
    /// Every production worker can drain and reconcile accepted controls. Stage
    /// and platform gates apply only when a held control is released.
    pub(crate) fn production(dependencies: ProductionCommandDependencies) -> Arc<Self> {
        Arc::new(Self {
            source_guard: dependencies.source_guard,
            authority: dependencies.authority,
            access: dependencies.access,
            store: dependencies.store,
            operations: dependencies.operations,
            context: dependencies.context,
        })
    }
}
