//! Bounded, authorized Agent chronology beside the standard Responses stream.
use axum::{routing::get, Router};
use layer2_core::sandbox::{agent_io::AgentCommandService, AgentReadService, AgentUpdateRecorder};
use std::sync::Arc;

#[cfg(test)]
mod binding_tests;
mod cursor;
mod eligibility;
mod error;
#[cfg(test)]
pub(crate) mod fixtures;
mod head;
#[cfg(test)]
mod head_tests;
#[cfg(test)]
mod message_binding_tests;
mod parse;
mod projection;
#[cfg(test)]
mod projection_tests;
mod shapes;
mod snapshot;
mod stream;
mod stream_live;
#[cfg(test)]
mod tests;

pub use eligibility::UiTransportDependencies;

#[derive(Clone)]
pub struct AgentV2State {
    reads: Arc<AgentReadService>,
    commands: Arc<AgentCommandService>,
    updates: Option<Arc<dyn AgentUpdateRecorder>>,
    ui_flag: bool,
    dependencies: UiTransportDependencies,
}

impl AgentV2State {
    pub fn new(reads: Arc<AgentReadService>, commands: Arc<AgentCommandService>) -> Self {
        Self {
            reads,
            commands,
            updates: None,
            ui_flag: false,
            dependencies: Default::default(),
        }
    }
    pub fn with_updates(mut self, updates: Arc<dyn AgentUpdateRecorder>) -> Self {
        self.updates = Some(updates);
        self
    }
    pub fn with_ui_transport(
        mut self,
        enabled: bool,
        dependencies: UiTransportDependencies,
    ) -> Self {
        self.ui_flag = enabled;
        self.dependencies = dependencies;
        self
    }
}

pub fn router(state: AgentV2State) -> Router {
    Router::new()
        .route("/api/v2/ui/transport", get(eligibility::read))
        .route("/api/v2/agents/{agent_id}/events/head", get(head::read))
        .route(
            "/api/v2/agents/{agent_id}/events/snapshot",
            get(snapshot::read),
        )
        .route("/api/v2/agents/{agent_id}/events", get(stream::read))
        .with_state(state)
}

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    #[derive(OpenApi)]
    #[openapi(
        paths(eligibility::read, head::read, snapshot::read, stream::read),
        components(schemas(
            shapes::EventHead,
            shapes::EventPage,
            shapes::EventItem,
            shapes::ResponseBinding,
            shapes::DurableEvent,
            eligibility::UiTransportView
        ))
    )]
    struct Api;
    Api::openapi()
}
