use super::AgentProjectionServices;
use axum::Router;
use layer1_public::rest::{agent_router, AgentRoutesState};
use layer2_core::OperationStore;
use std::sync::Arc;

impl AgentProjectionServices {
    #[cfg(feature = "qualification-hooks")]
    pub(crate) fn with_qualification(
        mut self,
        registry: Option<cma_qualification::Registry>,
    ) -> Self {
        self.qualification = registry.map(Arc::new);
        self
    }

    pub(crate) fn router(&self, operation_records: Arc<dyn OperationStore>) -> Router {
        let agent_routes = AgentRoutesState::new(
            self.agent_page_origin.clone(),
            Arc::clone(&self.agents),
            Arc::clone(&self.reads),
            Arc::clone(&self.operations),
            operation_records,
        );
        let agent_routes = match &self.owner_agent_restart {
            Some(service) => agent_routes.with_owner_agent_restart(service.clone()),
            None => agent_routes,
        };
        #[cfg(feature = "qualification-hooks")]
        let agent_routes = match &self.qualification {
            Some(registry) => agent_routes.with_control_checkpoint(registry.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.message_feedback {
            Some(store) => agent_routes.with_message_feedback(store.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.suggestions {
            Some(store) => agent_routes.with_suggestions(store.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.skill_support {
            Some(support) => agent_routes.with_skill_support(support.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.lifecycle {
            Some(lifecycle) => agent_routes.with_lifecycle(lifecycle.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.lifecycle_operations {
            Some(reader) => agent_routes.with_lifecycle_operations(reader.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.history_exporter {
            Some(exporter) => agent_routes.with_history_exporter(exporter.clone()),
            None => agent_routes,
        };
        let agent_routes = match &self.commands {
            Some(commands) => agent_routes.with_commands(commands.clone()),
            None => agent_routes,
        };
        let agent_routes = if self.sandbox_profile_readability {
            agent_routes.with_sandbox_profile_readability(self.sandbox_profiles.clone())
        } else {
            agent_routes
        };
        Router::new()
            .merge(agent_router(agent_routes))
            .merge(layer1_public::pty::router(self.reads.clone()))
    }
}
