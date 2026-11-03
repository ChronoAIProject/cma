//! Agent conversations projected into AG-UI. Threads are created through REST.

use crate::{InterfaceProfile, Operation};
use axum::{
    routing::{get, post},
    Router,
};

mod capabilities;
mod events;
mod launch_companion;
mod live;
mod replay;
mod run;
mod sandbox_companion;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod completion_tests;

pub use launch_companion::{launch_companion_router, LaunchCompanionState};

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    #[derive(OpenApi)]
    #[openapi(paths(
        capabilities::declare,
        run::run,
        replay::replay,
        sandbox_companion::snapshot
    ))]
    struct Api;
    Api::openapi()
}

#[cfg(test)]
mod launch_companion_tests;

pub fn router(state: crate::AgentConversationState) -> Router {
    Router::new()
        .route("/api/v1/agui/capabilities", get(capabilities::declare))
        .route("/api/v1/agui/run", post(run::run))
        .route("/api/v1/agui/events", get(replay::replay))
        .route(
            "/api/v1/agui/agents/{id}/sandbox",
            get(sandbox_companion::snapshot),
        )
        .with_state(state)
}

pub struct AguiInterface;

impl InterfaceProfile for AguiInterface {
    fn interface_id(&self) -> &'static str {
        "ag-ui"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::Discover,
            Operation::SendAgentInput,
            Operation::StreamAgentEvents,
            Operation::ResolveAgentApproval,
            Operation::InterruptAgent,
            Operation::SteerAgent,
            Operation::ReadAgentUsage,
        ]
    }
}
