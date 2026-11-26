use axum::{
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use layer3_adapter::wire::{ResponseFrame, WireOperation};
use layer3_adapter::{
    RuntimeStatus, RUNTIME_LEDGER_STATUS_MEDIA_TYPE, RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE,
};

use crate::supervisor::Supervisor;

pub(super) async fn response(
    supervisor: &Supervisor,
    headers: &HeaderMap,
    mut status: RuntimeStatus,
) -> Response {
    // Existing status readers deny unknown fields. New envelopes require an explicit Accept value.
    let accepts = |media_type| {
        headers
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|value| value.trim() == media_type)
    };
    status.agent_skill_upgrade =
        accepts(layer3_adapter::wire::AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE);
    status.agent_resource_preparation =
        accepts(layer3_adapter::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE);
    status.agent_browser = accepts(layer3_adapter::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE);
    status.agent_process_restart = supervisor.agent_process_restart_enabled()
        && accepts(layer3_adapter::wire::AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE);
    status.exact_github_credential_route =
        accepts(layer3_adapter::wire::GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE)
            && supervisor.supports_exact_github_credential_route();
    status.github_repository_scope = status.exact_github_credential_route
        && accepts(layer3_adapter::wire::GITHUB_REPOSITORY_SCOPE_STATUS_MEDIA_TYPE);
    if accepts(RUNTIME_LEDGER_STATUS_MEDIA_TYPE) {
        return match supervisor.ledger_observation(status).await {
            Ok(observation) => (
                [(header::CONTENT_TYPE, RUNTIME_LEDGER_STATUS_MEDIA_TYPE)],
                Json(observation),
            )
                .into_response(),
            Err(error) => Json(error.response(WireOperation::RuntimeStatus)).into_response(),
        };
    }
    if accepts(RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE) {
        return (
            [(
                header::CONTENT_TYPE,
                RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE,
            )],
            Json(supervisor.startup_policy_observation(status)),
        )
            .into_response();
    }
    Json(ResponseFrame::RuntimeStatus(status)).into_response()
}
