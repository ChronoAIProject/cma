//! Bounded, authorized Agent chronology beside the standard Responses stream.
use axum::{routing::get, Router};
use layer2_core::sandbox::{agent_io::AgentCommandService, AgentReadService, AgentUpdateRecorder};
use std::sync::Arc;

#[cfg(test)]
mod binding_tests;
mod buffered;
#[cfg(test)]
mod buffered_tests;
mod cursor;
mod eligibility;
mod error;
mod external_approval;
#[cfg(test)]
pub(crate) mod fixtures;
mod head;
#[cfg(test)]
mod head_tests;
#[cfg(test)]
mod message_binding_tests;
#[cfg(test)]
mod operation_replay_tests;
#[cfg(test)]
mod output_tests;
mod parse;
mod projection;
#[cfg(test)]
mod projection_tests;
mod shapes;
mod snapshot;
mod stream;
mod stream_live;
mod stream_wake;
mod task_experience;
#[cfg(test)]
mod task_experience_tests;
#[cfg(test)]
mod tests;

pub use eligibility::UiTransportDependencies;

#[derive(Clone)]
pub struct AgentV2State {
    reads: Arc<AgentReadService>,
    commands: Arc<AgentCommandService>,
    updates: Option<Arc<dyn AgentUpdateRecorder>>,
    ui_flag: bool,
    single_fence_pages: bool,
    event_wake: bool,
    fallback_poll: std::time::Duration,
    dependencies: UiTransportDependencies,
    task_experience: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    external_review_origin: external_approval::ReviewOrigin,
}

impl AgentV2State {
    pub fn new(reads: Arc<AgentReadService>, commands: Arc<AgentCommandService>) -> Self {
        Self {
            reads,
            commands,
            updates: None,
            ui_flag: false,
            single_fence_pages: false,
            event_wake: false,
            fallback_poll: std::time::Duration::from_secs(1),
            dependencies: Default::default(),
            task_experience: Default::default(),
            external_review_origin: Default::default(),
        }
    }
    pub fn with_single_fence_pages(mut self, enabled: bool) -> Self {
        self.single_fence_pages = enabled;
        self
    }
    pub fn with_external_approval_review_origin(mut self, origin: &str) -> Self {
        self.external_review_origin = external_approval::ReviewOrigin::configured(origin);
        self
    }
    pub fn with_event_wake(mut self, enabled: bool, fallback: std::time::Duration) -> Self {
        self.event_wake = enabled;
        self.fallback_poll = fallback.max(std::time::Duration::from_millis(100));
        self
    }
    pub fn with_updates(mut self, updates: Arc<dyn AgentUpdateRecorder>) -> Self {
        self.updates = Some(updates);
        self
    }
    pub fn with_task_experience(
        mut self,
        policy: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Self {
        self.task_experience = policy;
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
        .route("/api/v2/ui/task-experience", get(task_experience::read))
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
        paths(
            eligibility::read,
            task_experience::read,
            head::read,
            snapshot::read,
            stream::read
        ),
        components(schemas(
            shapes::EventHead,
            shapes::EventPage,
            shapes::EventItem,
            shapes::ResponseBinding,
            shapes::DurableEvent,
            eligibility::UiTransportView,
            task_experience::TaskExperienceView,
            task_experience::TaskExperienceCapabilities
        ))
    )]
    struct Api;
    Api::openapi()
}
