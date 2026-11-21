use super::resource_access_problem::ResourceProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::{
    resource_access::{ResourceAccessError, ResourceAccessStoreError},
    sandbox::{
        channel_usage::AgentChannelUsageService,
        store::{channel_usage_cursor, PageRequest},
    },
    AgentId,
};
use serde::Deserialize;
use std::sync::Arc;

mod shapes;
pub(crate) use shapes::*;

pub fn agent_channel_usage_router(service: Arc<AgentChannelUsageService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/channel-usage", get(read))
        .route("/api/v1/agents/{id}/channel-usage/history", get(history))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

/// Legacy explicit Bridge usage projection. Provider conversation admissions
/// are not recorded here; use provider-associations for retained membership.
/// Empty usage does not establish that an Agent has never been associated.
#[utoipa::path(get,path="/api/v1/agents/{id}/channel-usage",tag="agents",
    params(("id"=String,Path)),responses((status=200,body=AgentChannelUsageView),(status=404,body=crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(service): State<Arc<AgentChannelUsageService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Response {
    let id = AgentId(id);
    let result = match resource {
        Some(resource) => {
            service
                .read(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                    None,
                )
                .await
        }
        None => service.read_personal(&caller, &id, None).await,
    };
    match result {
        Ok(value) => Json(AgentChannelUsageView::from(value)).into_response(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryParams {
    limit: Option<u16>,
    cursor: Option<String>,
}
#[utoipa::path(get,path="/api/v1/agents/{id}/channel-usage/history",tag="agents",
    params(("id"=String,Path),("limit"=Option<u16>,Query),("cursor"=Option<String>,Query)),
    responses((status=200,body=AgentChannelUsageHistoryView),(status=403,body=crate::rest::ProblemDocument),(status=404,body=crate::rest::ProblemDocument)))]
pub(crate) async fn history(
    State(service): State<Arc<AgentChannelUsageService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    params: Result<Query<HistoryParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let page = match params {
        Ok(Query(params)) => PageRequest::new(params.limit.unwrap_or(50), params.cursor),
        Err(_) => return invalid_query(),
    };
    let page = match page {
        Ok(page) if channel_usage_cursor(&page).is_ok() => page,
        _ => return invalid_query(),
    };
    let id = AgentId(id);
    let result = match resource {
        Some(resource) => {
            service
                .read(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                    Some(&page),
                )
                .await
        }
        None => service.read_personal(&caller, &id, Some(&page)).await,
    };
    match result {
        Ok(value) => match value.history {
            Some(page) => Json(AgentChannelUsageHistoryView {
                agent_id: value.agent_id.to_string(),
                items: page.items.into_iter().map(Into::into).collect(),
                next: page.next,
            })
            .into_response(),
            None => ResourceProblem(ResourceAccessStoreError::Corrupt.into()).into_response(),
        },
        Err(ResourceAccessError::Store(ResourceAccessStoreError::Invalid)) => invalid_query(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}
fn invalid_query() -> Response {
    super::problem_response(
        StatusCode::BAD_REQUEST,
        super::problem_document(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "Channel history requires a limit from 1 through 200 and a positive revision cursor.",
            "invalid_query",
        ),
    )
}
