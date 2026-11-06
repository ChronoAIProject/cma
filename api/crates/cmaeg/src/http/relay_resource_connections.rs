//! Optional discovery; explicit saved bindings remain valid without this route.
use crate::{
    auth::{Creator, CreatorAuthState, CreatorAuthenticator},
    connections::relay_resources::{
        DiscoveryError, ResourceConnectionList, ResourceConnections, ResourcePlatform,
    },
};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use cmaeg_core::problem::ProblemCode;
use serde::Deserialize;
use utoipa::IntoParams;

impl CreatorAuthState for ResourceConnections {
    fn creator_auth(&self) -> &CreatorAuthenticator {
        &self.creator_auth
    }
}

pub fn router(state: ResourceConnections) -> Router {
    Router::new()
        .route("/v1/relay-resource-connections", get(list))
        .route_layer(axum::middleware::from_fn(reject_selected_owner))
        .with_state(state)
        .layer(axum::middleware::from_fn(super::problems::protocol_headers))
}

async fn reject_selected_owner(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // This personal selector rejects organization selection before the shared
    // creator extractor would make an organization-authority lookup.
    if request.headers().contains_key("x-cmaeg-owner") {
        return failure(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "Connection discovery uses the authenticated subject; select other bindings manually.",
        );
    }
    next.run(request).await
}

#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    platform: ResourcePlatform,
}

#[utoipa::path(get, path="/v1/relay-resource-connections", operation_id="listRelayResourceConnections", tag="connections", security(("creatorBearer"=[])), params(ListQuery), responses((status=200,body=ResourceConnectionList),(status="default",body=cmaeg_core::problem::Problem)))]
pub async fn list(
    State(state): State<ResourceConnections>,
    _creator: Creator,
    headers: HeaderMap,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return failure(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "Choose lark or feishu for connection discovery.",
        );
    };
    let Some(token) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return failure(
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "A valid NyxID bearer token is required.",
        );
    };
    match state.list(token, query.platform).await {
        Ok(list) => super::problems::no_store(Json(list).into_response()),
        Err(DiscoveryError::Unauthenticated) => failure(
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "NyxID connection discovery requires a current credential.",
        ),
        Err(DiscoveryError::Forbidden) => failure(
            StatusCode::FORBIDDEN,
            ProblemCode::Forbidden,
            "This credential cannot read the required NyxID connection metadata.",
        ),
        Err(DiscoveryError::Unavailable | DiscoveryError::NotFound) => failure(
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::ProviderUnavailable,
            "Connection discovery is temporarily unavailable; existing bindings are retained.",
        ),
    }
}

fn failure(status: StatusCode, code: ProblemCode, message: &str) -> Response {
    super::problems::no_store(super::problems::problem(status, code, message))
}
