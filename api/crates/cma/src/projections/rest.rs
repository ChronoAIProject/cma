use super::AgentProjectionServices;
use axum::{
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
    routing::any,
    Router,
};
use layer1_public::rest::{agent_router, AgentRoutesState, HttpProblem};
use layer2_core::OperationStore;
use std::sync::Arc;

pub(crate) const REST_LINKS: HeaderValue = HeaderValue::from_static(
    "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\", <https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat>; rel=\"profile\", </openapi.json>; rel=\"service-desc\"",
);

pub(crate) fn rest_router(resource_routes: Router) -> Router {
    layer1_public::rest::router()
        .merge(resource_routes)
        .route("/api/v1", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/", any(|| async { HttpProblem::NotFound }))
        .route("/api/v1/{*path}", any(|| async { HttpProblem::NotFound }))
        .method_not_allowed_fallback(|| async { HttpProblem::MethodNotAllowed })
        .layer(axum::middleware::from_fn(rest_links))
}

pub(crate) async fn rest_links(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().append(header::LINK, REST_LINKS);
    response
}

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
