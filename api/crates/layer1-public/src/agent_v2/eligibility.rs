use super::AgentV2State;
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Composition evidence supplied by the router that installs these services.
#[derive(Debug, Clone, Copy, Default)]
pub struct UiTransportDependencies {
    pub responses: bool,
    pub controls: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct UiTransportView {
    object: &'static str,
    schema_version: u16,
    pub enabled: bool,
    selection: &'static str,
    input: &'static str,
    snapshot_schema: u16,
    event_schema: u16,
    control_schema: u16,
    live_fragments: bool,
    snapshot: &'static str,
    events: &'static str,
    controls: &'static str,
    command_status: &'static str,
}

pub(super) fn view(state: &AgentV2State) -> UiTransportView {
    UiTransportView {
        object: "cma.ui.transport",
        schema_version: 1,
        enabled: state.ui_flag
            && state.dependencies.responses
            && state.dependencies.controls
            && state.updates.is_some(),
        selection: "new_tabs",
        input: "responses_v2",
        snapshot_schema: 1,
        event_schema: 1,
        control_schema: 1,
        live_fragments: state.updates.is_some(),
        snapshot: "/api/v2/agents/{agent_id}/events/snapshot",
        events: "/api/v2/agents/{agent_id}/events",
        controls: "/api/v2/agents/{agent_id}/responses/{response_id}/controls",
        command_status: "/api/v2/commands/{command_id}",
    }
}

#[utoipa::path(get, path = "/api/v2/ui/transport", tag = "agent events v2",
    responses((status = 200, body = UiTransportView), (status = 401, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    _auth: AuthenticatedResourceCaller,
) -> Response {
    let mut response = Json(view(&state)).into_response();
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
