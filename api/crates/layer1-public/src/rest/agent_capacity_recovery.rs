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

pub fn agent_capacity_recovery_router(service: Arc<CapacityRecoveryPreflightService>) -> Router {
    Router::new()
        .route(
            "/api/v1/agents/{id}/capacity-recovery/dry-run",
            get(dry_run),
        )
        .with_state(service)
}

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
    let mut response = match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => InstanceProblem::ResourceAccess(error).into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
