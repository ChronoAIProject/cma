use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{
    bridge::{setup::*, BridgeError, BridgeId},
    resource_access::ResourceOwner,
    SubjectId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod capabilities;
pub(crate) use capabilities::*;
mod creation_target;
pub(crate) use creation_target::*;
mod mutations;
pub(super) mod problem;
pub(crate) mod shapes;
pub(crate) use mutations::*;
use problem::BridgeProblem;
use shapes::*;

pub fn bridge_router(service: Option<Arc<BridgeSetupService>>) -> Router {
    Router::new()
        .route("/api/v1/bridge-creation-target", get(creation_target))
        .route("/api/v1/bridge-connectors", get(choices))
        .route("/api/v1/bridge-connectors/capabilities", get(capabilities))
        .route("/api/v1/bridges", get(list).post(create))
        .route("/api/v1/bridges/{id}", get(read))
        .route(
            "/api/v1/bridges/{id}/configuration",
            get(configuration).put(update),
        )
        .route("/api/v1/bridges/{id}/commands", post(control))
        .route("/api/v1/bridges/{id}/validate", post(validate))
        .route(
            "/api/v1/bridges/{id}/credential",
            axum::routing::put(import),
        )
        .route("/api/v1/bridges/{id}/credential/revoke", post(revoke))
        .layer(axum::middleware::from_fn(private_headers))
        .layer(axum::extract::DefaultBodyLimit::max(32768))
        .with_state(service)
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
fn credential(caller: &AuthenticatedResourceCaller) -> Option<&str> {
    caller.credential.as_ref().map(DelegatedCredential::value)
}
fn respond<T: Serialize>(result: Result<T, BridgeError>, status: StatusCode) -> Response {
    match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => BridgeProblem(error).into_response(),
    }
}
fn body<T>(body: Result<RequestBody<T>, BodyProblem>) -> Result<T, Box<Response>> {
    body.and_then(|body| body.into_parts().0)
        .map_err(|error| Box::new(error.into_response()))
}
fn key(key: IdempotencyKey) -> Result<String, Box<Response>> {
    key.into_required_value()
        .map_err(|error| Box::new(error.into_response()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {
    organization: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerParams {
    organization: Option<String>,
}
fn owner(caller: &AuthenticatedResourceCaller, organization: Option<String>) -> ResourceOwner {
    organization.map_or_else(
        || ResourceOwner::Person {
            subject: caller.context.caller.subject.clone(),
        },
        |id| ResourceOwner::Organization {
            subject: SubjectId::new(id),
        },
    )
}
fn invalid_query() -> Response {
    super::problem_response(
        StatusCode::BAD_REQUEST,
        super::problem_document(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "Use a valid owner scope, a bridge cursor and a limit from 1 through 100.",
            "invalid_query",
        ),
    )
}
#[utoipa::path(get,path="/api/v1/bridges",tag="bridges",params(("organization"=Option<String>,Query),("cursor"=Option<String>,Query),("limit"=Option<u32>,Query)),responses((status=200,body=BridgePageShape),(status=403,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    params: Result<Query<ListParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let cursor = params.cursor.map(BridgeId);
    let owner = owner(&caller, params.organization);
    respond(
        service
            .list(
                &caller.context,
                credential(&caller),
                &owner,
                cursor.as_ref(),
                params.limit.unwrap_or(50),
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(get,path="/api/v1/bridge-connectors",tag="bridges",params(("organization"=Option<String>,Query)),responses((status=200,body=BridgeConnectorChoicesShape),(status=403,body=super::ProblemDocument),(status=503,body=super::ProblemDocument)))]
pub(crate) async fn choices(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    params: Result<Query<OwnerParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let owner = owner(&caller, params.organization);
    respond(
        service
            .choices(&caller.context, credential(&caller), &owner)
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(get,path="/api/v1/bridges/{id}",tag="bridges",params(("id"=String,Path)),responses((status=200,body=BridgeViewShape),(status=404,body=super::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        service
            .detail(&caller.context, credential(&caller), &BridgeId(id))
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(get,path="/api/v1/bridges/{id}/configuration",tag="bridges",params(("id"=String,Path)),responses((status=200,body=BridgeConfigurationShape),(status=403,body=super::ProblemDocument),(status=404,body=super::ProblemDocument)))]
pub(crate) async fn configuration(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        service
            .configuration(&caller.context, credential(&caller), &BridgeId(id))
            .await,
        StatusCode::OK,
    )
}

fn unavailable() -> Response {
    super::problem_response(
        StatusCode::SERVICE_UNAVAILABLE,
        super::problem_document(
            StatusCode::SERVICE_UNAVAILABLE,
            "bridge_setup_unavailable",
            "Bridge setup requires a configured NyxID provider and an HTTPS CMA public origin.",
            "bridge_setup_unavailable",
        ),
    )
}
