//! Public, principal-scoped measured usage reports; no source query passthrough.
mod initial;
mod initial_openapi;
mod openapi;
#[cfg(test)]
mod options_tests;
#[cfg(test)]
mod owner_allocation_tests;
mod schemas;
mod view;
mod view_openapi;
mod wire;
use crate::AuthenticatedCaller;
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Path, Query, State,
    },
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use layer2_core::sandbox_metrics::*;
pub(super) use openapi::document;
use serde::Deserialize;
use std::sync::Arc;
#[derive(Clone, Copy)]
struct Admin(bool);
type Service = State<Arc<MetricsService>>;
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

pub fn sandbox_metrics_router(service: Arc<MetricsService>) -> Router {
    let routes = Router::new()
        .merge(initial::routes())
        .route("/view", get(view::latest))
        .route("/reports", post(create))
        .route("/reports/{report}", get(poll))
        .route("/reports/{report}/sandboxes", get(page))
        .route("/reports/{report}/series", get(series))
        .route("/reports/{report}/sandboxes/{sandbox}", get(detail));
    Router::new()
        .nest(
            "/api/v1/usage/metrics",
            routes.clone().layer(Extension(Admin(false))),
        )
        .nest(
            "/api/v1/admin/usage/metrics",
            routes
                .route("/options", post(options))
                .route("/reports/{report}/owners", get(owners))
                .route(
                    "/reports/{report}/owner-allocations",
                    get(owner_allocations),
                )
                .layer(Extension(Admin(true))),
        )
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}
async fn options(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: std::result::Result<Json<OptionQuery>, JsonRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, true) {
        return problem(error);
    }
    let query = match query {
        Ok(Json(value)) => value,
        Err(_) => return invalid(),
    };
    respond(bounded(service.options(&caller, query)).await)
}
async fn create(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    input: std::result::Result<Json<ReportInput>, JsonRejection>,
) -> Response {
    let input = match input {
        Ok(Json(v)) => v,
        Err(_) => return invalid(),
    };
    match bounded(service.submit(&caller, admin, input, now())).await {
        Ok(report) => {
            let status = if report.status == ReportStatus::Ready {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            let mut response = (status, Json(wire::value(&report))).into_response();
            if status == StatusCode::ACCEPTED {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, "2".parse().unwrap());
            }
            response
        }
        Err(e) => problem(e),
    }
}
async fn poll(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    Path(id): Path<String>,
    query: std::result::Result<Query<Empty>, QueryRejection>,
) -> Response {
    if query.is_err() {
        return invalid();
    }
    respond(bounded(service.report(&caller, admin, &id, now())).await)
}
async fn page(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    Path(id): Path<String>,
    query: std::result::Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(v)) => v,
        Err(_) => return invalid(),
    };
    respond(bounded(service.page(&caller, admin, &id, query, now())).await)
}
async fn series(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    Path(id): Path<String>,
    query: std::result::Result<Query<SeriesQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(v)) => v,
        Err(_) => return invalid(),
    };
    respond(bounded(service.series(&caller, admin, &id, query, now())).await)
}
async fn detail(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    Path((report, id)): Path<(String, String)>,
    query: std::result::Result<Query<Empty>, QueryRejection>,
) -> Response {
    if query.is_err() {
        return invalid();
    }
    respond(bounded(service.detail(&caller, admin, &report, &id, now())).await)
}
async fn owners(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: std::result::Result<Query<OwnerQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(v)) => v,
        Err(_) => return invalid(),
    };
    respond(bounded(service.owners(&caller, &id, query, now())).await)
}
async fn owner_allocations(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: std::result::Result<Query<OwnerAllocationQuery>, QueryRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, true) {
        return problem(error);
    }
    let query = match query {
        Ok(Query(value)) => value,
        Err(_) => return invalid(),
    };
    respond(bounded(service.owner_allocations(&caller, &id, query, now())).await)
}
async fn bounded<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .map_err(|_| MetricsError::Unavailable)?
}
fn respond<T: serde::Serialize>(result: Result<T>) -> Response {
    match result {
        Ok(value) => Json(wire::value(&value)).into_response(),
        Err(error) => problem(error),
    }
}
fn invalid() -> Response {
    problem(MetricsError::Invalid(
        "Invalid typed usage query; arbitrary selectors and source parameters are not accepted."
            .into(),
    ))
}
/// Identifies the typed disabled-view response inside the server middleware.
#[derive(Clone, Copy)]
pub struct UsageReportViewDisabled;

fn problem(error: MetricsError) -> Response {
    let (status, code) = match &error {
        MetricsError::ViewDisabled => (StatusCode::NOT_IMPLEMENTED, "report_view_disabled"),
        MetricsError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_query"),
        MetricsError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
        MetricsError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        MetricsError::Expired => (StatusCode::GONE, "report_expired"),
        MetricsError::Busy => (StatusCode::TOO_MANY_REQUESTS, "reporting_budget_exceeded"),
        MetricsError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "usage_unavailable"),
    };
    let mut response = super::problem_response(
        status,
        super::problem_document(status, code, &error.to_string(), code),
    );
    if matches!(error, MetricsError::ViewDisabled) {
        response.extensions_mut().insert(UsageReportViewDisabled);
    }
    if matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "2".parse().unwrap());
    }
    response
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod view_tests;
