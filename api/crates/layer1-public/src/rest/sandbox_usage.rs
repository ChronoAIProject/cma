//! Public allocation usage API, shared by Settings and Operations.
use crate::AuthenticatedCaller;
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::store::{PageRequest, SandboxStoreError};
use layer2_core::sandbox_usage::{Allocation, SandboxUsage, UsageEvent, UsageTotals};
use layer2_core::sandbox_usage_service::{SandboxUsageService, UsageReadError};
use layer2_core::{SandboxId, SubjectId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

pub fn sandbox_usage_router(service: Arc<SandboxUsageService>) -> Router {
    Router::new()
        .route("/api/v1/usage", get(summary))
        .route("/api/v1/usage/sandboxes", get(list))
        .route("/api/v1/usage/sandboxes/{id}", get(read))
        .route("/api/v1/usage/sandboxes/{id}/events", get(events))
        .route("/api/v1/admin/usage", get(admin_summary))
        .route("/api/v1/admin/usage/sandboxes", get(admin_list))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsageTotalsView {
    uptime_seconds: u64,
    cpu_core_hours: f64,
    memory_gib_hours: f64,
    disk_gib_hours: f64,
    cpu_millis_seconds: u64,
    memory_mib_seconds: u64,
    disk_gib_seconds: u64,
}
impl From<UsageTotals> for UsageTotalsView {
    fn from(value: UsageTotals) -> Self {
        Self {
            uptime_seconds: value.uptime_seconds,
            cpu_core_hours: value.cpu_millis_seconds as f64 / 3_600_000.0,
            memory_gib_hours: value.memory_mib_seconds as f64 / 3_686_400.0,
            disk_gib_hours: value.disk_gib_seconds as f64 / 3_600.0,
            cpu_millis_seconds: value.cpu_millis_seconds,
            memory_mib_seconds: value.memory_mib_seconds,
            disk_gib_seconds: value.disk_gib_seconds,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AllocationView {
    running: bool,
    cpu_millis: u32,
    memory_mib: u32,
    disk_gib: u32,
}
impl From<Allocation> for AllocationView {
    fn from(value: Allocation) -> Self {
        Self {
            running: value.running,
            cpu_millis: value.cpu_millis,
            memory_mib: value.memory_mib,
            disk_gib: value.disk_gib,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxUsageView {
    sandbox_id: String,
    owner: String,
    name: String,
    status: String,
    allocation: AllocationView,
    created_at: u64,
    tracking_started_at: u64,
    as_of: u64,
    complete_lifecycle: bool,
    totals: UsageTotalsView,
}
impl SandboxUsageView {
    fn project(value: SandboxUsage, as_of: u64) -> Result<Self, UsageReadError> {
        let as_of = as_of.max(value.last_observed_at);
        let totals = value.totals_at(as_of).map_err(|error| {
            UsageReadError::Store(SandboxStoreError::Corrupt(error.to_string()))
        })?;
        let allocation = value.current_allocation();
        Ok(Self {
            sandbox_id: value.sandbox_id.to_string(),
            owner: value.owner.to_string(),
            name: value.name,
            status: value.status,
            allocation: allocation.into(),
            created_at: value.created_at,
            tracking_started_at: value.tracking_started_at,
            as_of,
            complete_lifecycle: value.complete_lifecycle,
            totals: totals.into(),
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsagePageView {
    items: Vec<SandboxUsageView>,
    next: Option<String>,
    as_of: u64,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsageSummaryView {
    as_of: u64,
    measurement: &'static str,
    sandbox_count: u64,
    running_count: u64,
    incomplete_lifecycle_count: u64,
    tracking_started_at: Option<u64>,
    totals: UsageTotalsView,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsageEventView {
    transaction_id: String,
    revision: u64,
    source_seq: u64,
    #[schema(value_type = String)]
    kind: layer2_core::sandbox_usage::UsageEventKind,
    started_at: u64,
    ended_at: u64,
    status: String,
    allocation: AllocationView,
    next_allocation: AllocationView,
    quantity: UsageTotalsView,
}
impl From<UsageEvent> for UsageEventView {
    fn from(value: UsageEvent) -> Self {
        Self {
            transaction_id: value.transaction_id(),
            revision: value.revision,
            source_seq: value.source_seq,
            kind: value.kind,
            started_at: value.started_at,
            ended_at: value.ended_at,
            status: value.status,
            allocation: value.allocation.into(),
            next_allocation: value.next_allocation.into(),
            quantity: value.quantity.into(),
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsageEventsView {
    items: Vec<UsageEventView>,
    next: Option<String>,
}

#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageQuery {
    owner: Option<String>,
    #[param(minimum = 1, maximum = 200, default = 50)]
    limit: Option<u16>,
    /// Continuation returned by this endpoint for the same caller and filters.
    /// Signed continuations expire 15 minutes after the first page. When
    /// snapshot pagination is enabled, continuations keep the same read view
    /// while its history is available. If the service returns invalid_query,
    /// restart without a cursor. Legacy cursors are accepted during transition.
    #[param(max_length = 4096)]
    cursor: Option<String>,
}
impl UsageQuery {
    fn page(&self) -> Result<PageRequest, UsageReadError> {
        Ok(PageRequest::new(
            self.limit.unwrap_or(50),
            self.cursor.clone(),
        )?)
    }
}
type Service = State<Arc<SandboxUsageService>>;

fn parse_query(
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Result<UsageQuery, UsageReadError> {
    query
        .map(|Query(query)| query)
        .map_err(|_| SandboxStoreError::InvalidPage.into())
}

/// Lifetime allocated resource-time for the authenticated owner.
#[utoipa::path(get, path="/api/v1/usage", tag="usage", responses((status=200,body=UsageSummaryView)))]
pub(crate) async fn summary(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Response {
    summary_response(&service, &caller, false, None).await
}
/// Deployment-wide totals; optionally restrict to one owner. Requires cma:admin:sandboxes.
#[utoipa::path(get, path="/api/v1/admin/usage", tag="usage", params(UsageQuery), responses((status=200,body=UsageSummaryView),
    (status=400,description="invalid_query: malformed or unknown query parameters",body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn admin_summary(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Response {
    let query = match parse_query(query) {
        Ok(query) => query,
        Err(error) => return problem(error),
    };
    summary_response(&service, &caller, true, query.owner.map(SubjectId).as_ref()).await
}
async fn summary_response(
    service: &SandboxUsageService,
    caller: &layer2_core::Caller,
    admin: bool,
    owner: Option<&SubjectId>,
) -> Response {
    let as_of = now();
    match service.summary(caller, admin, owner, as_of).await {
        Ok(value) => Json(UsageSummaryView {
            as_of,
            measurement: "allocated_resource_time",
            sandbox_count: value.sandbox_count,
            running_count: value.running_count,
            incomplete_lifecycle_count: value.incomplete_lifecycle_count,
            tracking_started_at: value.tracking_started_at,
            totals: value.totals.into(),
        })
        .into_response(),
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/usage/sandboxes", tag="usage", params(UsageQuery), responses((status=200,body=UsagePageView),
    (status=400,description="invalid_query: use a limit from 1 through 200; restart without a cursor if the continuation is invalid or expired",body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn list(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Response {
    let query = match parse_query(query) {
        Ok(query) => query,
        Err(error) => return problem(error),
    };
    list_response(&service, &caller, false, query).await
}
#[utoipa::path(get, path="/api/v1/admin/usage/sandboxes", tag="usage", params(UsageQuery), responses((status=200,body=UsagePageView),
    (status=400,description="invalid_query: use a limit from 1 through 200; restart without a cursor if the continuation is invalid or expired",body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn admin_list(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Response {
    let query = match parse_query(query) {
        Ok(query) => query,
        Err(error) => return problem(error),
    };
    list_response(&service, &caller, true, query).await
}
async fn list_response(
    service: &SandboxUsageService,
    caller: &layer2_core::Caller,
    admin: bool,
    query: UsageQuery,
) -> Response {
    let page = match query.page() {
        Ok(page) => page,
        Err(error) => return problem(error),
    };
    match service
        .list(caller, admin, query.owner.map(SubjectId).as_ref(), &page)
        .await
    {
        Ok(value) => {
            let as_of = value.as_of.unwrap_or_else(now);
            let items = value
                .page
                .items
                .into_iter()
                .map(|value| SandboxUsageView::project(value, as_of))
                .collect();
            match items {
                Ok(items) => Json(UsagePageView {
                    items,
                    next: value.page.next,
                    as_of,
                })
                .into_response(),
                Err(error) => problem(error),
            }
        }
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/usage/sandboxes/{id}", tag="usage", params(("id"=String,Path)), responses((status=200,body=SandboxUsageView)))]
pub(crate) async fn read(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match service
        .read(&caller, &SandboxId(id))
        .await
        .and_then(|value| SandboxUsageView::project(value, now()))
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/usage/sandboxes/{id}/events", tag="usage", params(("id"=String,Path),UsageQuery), responses((status=200,body=UsageEventsView),
    (status=400,description="invalid_query: use a limit from 1 through 200; restart without a cursor if the continuation is invalid or expired",body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn events(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Response {
    let query = match parse_query(query) {
        Ok(query) => query,
        Err(error) => return problem(error),
    };
    let page = match query.page() {
        Ok(page) => page,
        Err(error) => return problem(error),
    };
    match service
        .events(
            &caller,
            &SandboxId(id),
            query.owner.map(SubjectId).as_ref(),
            &page,
        )
        .await
    {
        Ok(value) => Json(UsageEventsView {
            items: value.items.into_iter().map(Into::into).collect(),
            next: value.next,
        })
        .into_response(),
        Err(error) => problem(error),
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn problem(error: UsageReadError) -> Response {
    let (status, code, detail) = match error {
        UsageReadError::InvalidCursor => (
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "Usage continuation is invalid or expired. Restart from the first page without a cursor.",
        ),
        UsageReadError::Forbidden => (
            StatusCode::FORBIDDEN,
            "forbidden",
            "Usage access is not permitted.",
        ),
        UsageReadError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "Sandbox usage was not found.",
        ),
        UsageReadError::Store(SandboxStoreError::InvalidPage) => (
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "Use a valid cursor and a limit from 1 through 200.",
        ),
        UsageReadError::Store(error) => {
            tracing::error!(error = %error, "sandbox usage read failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "usage_unavailable",
                "Usage is temporarily unavailable.",
            )
        }
    };
    super::problem_response(status, super::problem_document(status, code, detail, code))
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
pub(super) fn document() -> utoipa::openapi::OpenApi {
    #[derive(OpenApi)]
    #[openapi(paths(summary, list, read, events, admin_summary, admin_list),
        components(schemas(UsageTotalsView, AllocationView, SandboxUsageView, UsageSummaryView,
            UsagePageView, UsageEventView, UsageEventsView)),
        tags((name="usage", description="Sandbox allocation metering, uptime and lifecycle intervals")))]
    struct UsageApi;
    UsageApi::openapi()
}

#[cfg(test)]
#[path = "sandbox_usage_tests.rs"]
pub(crate) mod tests;
