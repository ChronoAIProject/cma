//! Read-only capacity recovery preflight. Admission remains independently gated.
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    sandbox::agent_capacity_recovery::preflight::CapacityRecoveryPreflightService, AgentId,
};

use super::instance_problem::InstanceProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

mod commands;
pub use commands::router as agent_capacity_recovery_commands_router;

pub fn agent_capacity_recovery_router(service: Arc<CapacityRecoveryPreflightService>) -> Router {
    Router::new()
        .route(
            "/api/v1/agents/{id}/capacity-recovery/dry-run",
            get(dry_run),
        )
        .with_state(service)
        .layer(axum::middleware::map_response(private_no_store))
}

/// Observe current authorized metadata without admitting recovery or reading history.
#[utoipa::path(get, path = "/api/v1/agents/{id}/capacity-recovery/dry-run", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = serde_json::Value), (status = 401),
        (status = 403), (status = 404), (status = 503)))]
async fn dry_run(
    State(service): State<Arc<CapacityRecoveryPreflightService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Response {
    let id = AgentId(id);
    let result = match resource {
        Some(resource) => {
            service
                .read_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => service.read(&caller, &id).await,
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => InstanceProblem::ResourceAccess(error).into_response(),
    }
}

// Apply to extractor refusals too: unauthenticated/error responses must not be
// cached simply because the request never reached the handler.
async fn private_no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(dry_run))]
struct PreflightApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    let mut document = PreflightApi::openapi();
    document.merge(commands::document());
    document
}

#[cfg(test)]
mod tests;
