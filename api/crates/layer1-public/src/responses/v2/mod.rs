//! Versioned streaming Responses subset backed by durable Agent commands.

use axum::{
    routing::{get, post},
    Router,
};
use layer2_core::sandbox::{agent_io::AgentCommandService, AgentReadService};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[cfg(test)]
mod activity_tests;
#[cfg(test)]
pub(crate) mod binding_tests;
mod bindings;
pub(crate) mod controls;
mod conversation;
mod error;
mod event_context;
#[cfg(test)]
mod event_context_tests;
#[cfg(test)]
mod existing_binding_tests;
mod input_receipt;
pub(crate) mod interruption;
#[cfg(test)]
mod model_tests;
mod output;
#[cfg(test)]
mod output_gap_tests;
mod parse;
pub(super) mod projection;
pub(crate) mod queued_cancellation;
pub(crate) mod queued_input;
mod routes;
mod routing;
#[cfg(test)]
mod routing_tests;
pub use routing::{BindingEventRouter, BindingRouteError};
mod sender_context;
mod shared_projection;
mod stream;
mod stream_pages;
#[cfg(test)]
pub(crate) mod tests;

pub use error::{missing as resource_not_found, unauthenticated as authentication_required};
pub use output::{ActivityItem, V2OutputItem};
pub use projection::{ResponseConversation, V2ResponseObject, V2ResponseStatus};
pub use queued_cancellation::QueuedCancellationNeedsOwner;

#[derive(Clone)]
pub struct ResponsesV2State {
    pub(crate) agent_page_origin: crate::agent_page::AgentPageOrigin,
    commands: Arc<AgentCommandService>,
    reads: Arc<AgentReadService>,
    bindings: Option<Arc<layer2_core::sandbox::binding::BindingAccessService>>,
    event_router: Option<Arc<dyn BindingEventRouter>>,
    paged_events: bool,
    retained_context_lookup: bool,
    durable_recovery_slots: Arc<Semaphore>,
    #[cfg(any(test, feature = "test-support"))]
    replay_refresh_barrier: Option<(u64, tokio::sync::watch::Receiver<bool>)>,
}
impl ResponsesV2State {
    pub fn with_paged_events(mut self, enabled: bool) -> Self {
        self.paged_events = enabled;
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_replay_refresh_barrier(
        mut self,
        after: u64,
        start: tokio::sync::watch::Receiver<bool>,
    ) -> Self {
        self.replay_refresh_barrier = Some((after, start));
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn wait_for_replay_refresh(&mut self, next: u64) {
        if let Some((_, mut start)) = self
            .replay_refresh_barrier
            .take_if(|(after, _)| next > *after)
        {
            start
                .wait_for(|ready| *ready)
                .await
                .expect("replay fixture must explicitly release refreshes");
        }
    }

    pub fn with_event_router(mut self, router: Arc<dyn BindingEventRouter>) -> Self {
        self.event_router = Some(router);
        self
    }
    pub fn with_bindings(
        mut self,
        bindings: Arc<layer2_core::sandbox::binding::BindingAccessService>,
    ) -> Self {
        self.bindings = Some(bindings);
        self
    }

    /// Enables the single-read retained-context path only for an explicit
    /// rollout. The legacy path remains the default until its security and
    /// latency evidence is accepted.
    pub fn with_retained_context_lookup(mut self, enabled: bool) -> Self {
        self.retained_context_lookup = enabled;
        self
    }

    pub fn new(
        agent_page_origin: crate::agent_page::AgentPageOrigin,
        commands: Arc<AgentCommandService>,
        reads: Arc<AgentReadService>,
    ) -> Self {
        Self {
            agent_page_origin,
            commands,
            reads,
            bindings: None,
            event_router: None,
            paged_events: false,
            retained_context_lookup: false,
            // Independent polling costs more than a shared observation.
            durable_recovery_slots: Arc::new(Semaphore::new(16)),
            #[cfg(any(test, feature = "test-support"))]
            replay_refresh_barrier: None,
        }
    }
}
pub fn router(state: ResponsesV2State) -> Router {
    Router::new()
        .route("/api/v2", get(routes::capabilities))
        .route("/api/v2/responses", post(routes::create))
        .route(
            "/api/v2/agents/{agent_id}/responses",
            post(routes::create_alias),
        )
        .route(
            "/api/v2/agents/{agent_id}/responses/receipt",
            post(input_receipt::lookup).layer(axum::middleware::map_response(
                crate::conversation::commands::no_store_receipt,
            )),
        )
        .route(
            "/api/v2/agents/{agent_id}/queued-inputs",
            get(queued_input::list),
        )
        .route(
            "/api/v2/agents/{agent_id}/responses/{response_id}/queued-input",
            post(queued_input::submit),
        )
        .route(
            "/api/v2/agents/{agent_id}/responses/{response_id}/interrupt",
            post(interruption::submit),
        )
        .route("/api/v2/responses/{response_id}", get(routes::retrieve))
        .route(
            "/api/v2/responses/{response_id}/retained-output",
            get(routes::retained_output::read),
        )
        .route(
            "/api/v2/responses/{response_id}/output",
            get(routes::output::output_page),
        )
        .route("/api/v2/commands/{command_id}", get(controls::status))
        .route(
            "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
            get(controls::observe).post(controls::submit),
        )
        .route(
            "/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
            get(queued_cancellation::observe).post(queued_cancellation::submit),
        )
        .route(
            "/api/v2/responses/{response_id}/events",
            get(routes::replay),
        )
        .route(
            "/api/v2/conversations/{conversation_id}",
            get(conversation::retrieve),
        )
        .route(
            "/api/v2/conversations/{conversation_id}/items",
            get(conversation::items),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}",
            get(event_context::retrieve),
        )
        .with_state(state)
}
pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    #[derive(OpenApi)]
    #[openapi(
        paths(
            event_context::retrieve,
            routes::capabilities,
            routes::create,
            routes::create_alias,
            input_receipt::lookup,
            routes::retrieve,
            routes::output::output_page,
            routes::retained_output::read,
            routes::replay,
            controls::observe,
            controls::submit,
            interruption::submit,
            controls::status,
            queued_cancellation::observe,
            queued_cancellation::submit,
            queued_input::list,
            queued_input::submit,
            conversation::retrieve,
            conversation::items
        ),
        components(schemas(V2ResponseObject))
    )]
    struct Api;
    Api::openapi()
}

#[cfg(test)]
mod reset_tests;
