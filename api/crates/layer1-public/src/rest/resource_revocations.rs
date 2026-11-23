use axum::{
    extract::{Path, Query},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use layer2_core::resource_access::{
    execution::ExecutionError,
    revocations::{ResourceRevocations, RevocationStage, RevocationStatus},
    ResourceAccessService, ResourceSelector, ShareChange, ShareCommand,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use super::{
    idempotency::{IdempotencyKey, RequestBody},
    resource_access_shapes::*,
    BodyProblem,
};
use crate::AuthenticatedResourceCaller;

pub(super) type Revocations = Extension<Arc<dyn ResourceRevocations>>;

#[derive(Serialize, ToSchema)]
pub(crate) struct RevocationReferenceView {
    operation_id: String,
    kind: String,
    id: String,
    share_id: String,
    expected_revision: u64,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RevocationAwareSuccess<T> {
    #[serde(flatten)]
    value: T,
    /// Original status identity. A successful access change does not prove external effects ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    revocation: Option<RevocationReferenceView>,
}

pub(super) fn completed<T: Serialize>(status: &RevocationStatus, value: T) -> Response {
    let (kind, id) = match &status.selector {
        ResourceSelector::Agent { agent_id } => ("agent", agent_id.as_str()),
        ResourceSelector::Sandbox { sandbox_id } => ("sandbox", sandbox_id.as_str()),
    };
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(RevocationAwareSuccess {
            value,
            revocation: Some(RevocationReferenceView {
                operation_id: status.operation_id.clone(),
                kind: kind.into(),
                id: id.into(),
                share_id: status.share_id.clone(),
                expected_revision: status.expected_revision,
            }),
        }),
    )
        .into_response()
}

pub(super) fn routes() -> Router<Arc<ResourceAccessService>> {
    Router::new()
        .route("/api/v1/resource-revocations", post(create))
        .route("/api/v1/resource-revocations/{operation_id}", get(status))
}

/// Install only when composition has verified complete participant coverage.
pub fn resource_access_router_with_revocations(
    access: Arc<ResourceAccessService>,
    revocations: Arc<dyn ResourceRevocations>,
) -> Router {
    super::resource_access::resource_access_router(access).layer(Extension(revocations))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevocationBody {
    pub kind: String,
    pub id: String,
    pub share_id: String,
    pub expected_revision: u64,
    pub change: RevocationChange,
}
#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RevocationChange {
    Revoke,
    SetMode { mode: ShareModeView },
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevocationQuery {
    pub kind: String,
    pub id: String,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RevocationStateView {
    Pending,
    EvidenceReceived,
    Effective,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct RevocationStatusView {
    operation_id: String,
    selector: ResourceSelectorView,
    share_id: String,
    expected_revision: u64,
    state: RevocationStateView,
    participants: u32,
    stopped: u32,
    /// Submitted external effects without a terminal observation. Effective does not imply rollback.
    unresolved_effects: u32,
    deadline_unix_ms: u64,
    /// A missed deadline is an incomplete outcome, never evidence that access was removed.
    deadline_exceeded: bool,
    receipt: Option<ShareReceiptView>,
}
impl From<RevocationStatus> for RevocationStatusView {
    fn from(value: RevocationStatus) -> Self {
        Self {
            operation_id: value.operation_id,
            selector: value.selector.into(),
            share_id: value.share_id,
            expected_revision: value.expected_revision,
            state: match value.stage {
                RevocationStage::Pending => RevocationStateView::Pending,
                RevocationStage::EvidenceReceived => RevocationStateView::EvidenceReceived,
                RevocationStage::Effective => RevocationStateView::Effective,
            },
            participants: value.participants,
            stopped: value.stopped,
            unresolved_effects: value.unresolved_effects,
            deadline_unix_ms: value.deadline_unix_ms,
            deadline_exceeded: value.deadline_exceeded,
            receipt: value.receipt.map(Into::into),
        }
    }
}

pub(super) fn problem(error: ExecutionError) -> Response {
    let (status, code, detail) = match error {
        ExecutionError::AuthorityChanged => (StatusCode::NOT_FOUND, "resource_not_found", "Resource not found."),
        ExecutionError::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_key_reused", "The key was used for another share command."),
        ExecutionError::Closing { .. } => (StatusCode::CONFLICT, "resource_revocation_conflict", "Another share change is pending. Keep the original command key."),
        ExecutionError::Capacity => (StatusCode::TOO_MANY_REQUESTS, "resource_revocation_capacity", "Revocation capacity is temporarily unavailable. Retry the same command key."),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "resource_revocation_unavailable", "Revocation could not be confirmed. Keep the original command key and check its status or retry."),
    };
    super::problem_response(status, super::problem_document(status, code, detail, code))
}

pub(super) fn pending(value: &RevocationStatus) -> Response {
    let status = StatusCode::CONFLICT;
    let mut response = super::problem_response(
        status,
        super::problem_document(
            status,
            "resource_revocation_pending",
            "Revocation is pending. Access removal has not been confirmed.",
            "resource_revocation_pending",
        ),
    );
    let (kind, id) = match &value.selector {
        ResourceSelector::Agent { agent_id } => ("agent", agent_id.as_str()),
        ResourceSelector::Sandbox { sandbox_id } => ("sandbox", sandbox_id.as_str()),
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("kind", kind)
        .append_pair("id", id)
        .finish();
    if value.operation_id.len() == 64 && value.operation_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        if let Ok(link) = HeaderValue::from_str(&format!(
            "</api/v1/resource-revocations/{}?{query}>; rel=\"status\"",
            value.operation_id,
        )) {
            response.headers_mut().insert(header::LINK, link);
        }
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[utoipa::path(post, path="/api/v1/resource-revocations", tag="resource-access",
    params(("Idempotency-Key"=String, Header)), request_body=RevocationBody,
    responses((status=202,body=RevocationStatusView),(status=400,body=super::ProblemDocument,content_type="application/problem+json"),(status=401,body=super::ProblemDocument,content_type="application/problem+json"),(status=422,body=super::ProblemDocument,content_type="application/problem+json"),(status=429,body=super::ProblemDocument,content_type="application/problem+json"),(status=404,body=super::ProblemDocument,content_type="application/problem+json"),(status=409,body=super::ProblemDocument,content_type="application/problem+json"),(status=503,body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn create(
    revocations: Option<Revocations>,
    caller: AuthenticatedResourceCaller,
    key: IdempotencyKey,
    body: Result<RequestBody<RevocationBody>, BodyProblem>,
) -> Response {
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let key = match key.into_required_value() {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let selector = match super::resource_access::selector(&body.kind, body.id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    if body.share_id.trim().is_empty()
        || body.share_id.len() > 128
        || body.expected_revision == 0
        || body.expected_revision > i64::MAX as u64
    {
        return super::resource_access_problem::ResourceProblem(
            layer2_core::resource_access::ResourceAccessStoreError::Invalid.into(),
        )
        .into_response();
    }
    let Some(Extension(service)) = revocations else {
        return problem(ExecutionError::Disabled);
    };
    match service
        .request(
            &caller.context,
            caller.credential.as_ref().map(|c| c.value()),
            ShareCommand {
                selector,
                change: match body.change {
                    RevocationChange::Revoke => ShareChange::Revoke {
                        share_id: body.share_id,
                    },
                    RevocationChange::SetMode { mode } => ShareChange::SetMode {
                        share_id: body.share_id,
                        mode: mode.into(),
                    },
                },
                expected_share_revision: Some(body.expected_revision),
                idempotency_key: key,
            },
        )
        .await
    {
        Ok(value) => (
            StatusCode::ACCEPTED,
            [(header::CACHE_CONTROL, "no-store")],
            Json(RevocationStatusView::from(value)),
        )
            .into_response(),
        Err(error) => problem(error),
    }
}

#[utoipa::path(get, path="/api/v1/resource-revocations/{operation_id}", tag="resource-access",
    params(("operation_id"=String, Path),("kind"=String, Query),("id"=String, Query)),
    responses((status=200,body=RevocationStatusView),(status=400,body=super::ProblemDocument,content_type="application/problem+json"),(status=401,body=super::ProblemDocument,content_type="application/problem+json"),(status=404,body=super::ProblemDocument,content_type="application/problem+json"),(status=503,body=super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn status(
    revocations: Option<Revocations>,
    caller: AuthenticatedResourceCaller,
    Path(operation): Path<String>,
    query: Result<Query<RevocationQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(value)) => value,
        Err(_) => return problem(ExecutionError::AuthorityChanged),
    };
    let selector = match super::resource_access::selector(&query.kind, query.id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let Some(Extension(service)) = revocations else {
        return problem(ExecutionError::Disabled);
    };
    match service
        .status(
            &caller.context,
            caller.credential.as_ref().map(|c| c.value()),
            &selector,
            &operation,
        )
        .await
    {
        Ok(value) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(RevocationStatusView::from(value)),
        )
            .into_response(),
        Err(error) => problem(error),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(create, status),
    components(schemas(
        RevocationBody,
        RevocationChange,
        RevocationStatusView,
        RevocationStateView,
        RevocationQuery,
    ))
)]
struct ApiDoc;
pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    ApiDoc::openapi()
}
