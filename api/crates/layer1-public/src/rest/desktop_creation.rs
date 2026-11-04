use axum::{
    extract::{Extension, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::desktop_creation::{DesktopAvailability, DesktopCapability};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use super::{AgentLifecycleProvisioningError, AgentRoutesState};
use crate::AuthenticatedCaller;

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DesktopCreationScope {
    pub workspace_id: Option<String>,
    pub sandbox_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DesktopAvailabilityView {
    Available,
    Unsupported,
    TemporarilyUnavailable,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DesktopCapabilityView {
    state: DesktopAvailabilityView,
    reason: String,
    runtime_contract: u32,
    min_memory_mib: u32,
    min_cpu_millis: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_image: Option<String>,
}

impl From<DesktopCapability> for DesktopCapabilityView {
    fn from(value: DesktopCapability) -> Self {
        Self {
            state: match value.state {
                DesktopAvailability::Available => DesktopAvailabilityView::Available,
                DesktopAvailability::Unsupported => DesktopAvailabilityView::Unsupported,
                DesktopAvailability::TemporarilyUnavailable => {
                    DesktopAvailabilityView::TemporarilyUnavailable
                }
            },
            reason: value.reason,
            runtime_contract: value.runtime_contract,
            min_memory_mib: value.min_memory_mib,
            min_cpu_millis: value.min_cpu_millis,
            runtime_image: value.runtime_image,
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(read),
    components(schemas(DesktopCapabilityView, DesktopCreationScope, DesktopAvailabilityView))
)]
struct DesktopApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    DesktopApi::openapi()
}

#[utoipa::path(get, path = "/api/v1/desktop-capability", tag = "agents", operation_id = "readDesktopCapability",
    params(("workspace_id" = Option<String>, Query), ("sandbox_id" = Option<String>, Query)),
    responses((status = 200, body = DesktopCapabilityView), (status = 400, body = super::ProblemDocument),
        (status = 401, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument),
        (status = 404, body = super::ProblemDocument), (status = 422, body = super::ProblemDocument),
        (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    context: Option<Extension<layer2_core::resource_access::CallerContext>>,
    query: Result<Query<DesktopCreationScope>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if !caller
        .authorization
        .allows(layer2_core::Capability::UseOwnSandboxes)
    {
        return AgentLifecycleProvisioningError::Forbidden.into_response();
    }
    let scope = match query {
        Ok(Query(scope)) => scope,
        Err(_) => return super::instance_problem::InstanceProblem::InvalidQuery.into_response(),
    };
    let Some(lifecycle) = state.lifecycle else {
        return AgentLifecycleProvisioningError::Unavailable.into_response();
    };
    lifecycle
        .desktop_capability(&caller, context.as_ref().map(|value| &value.0), scope)
        .await
        .map(|value| Json(DesktopCapabilityView::from(value)).into_response())
        .unwrap_or_else(IntoResponse::into_response)
}
