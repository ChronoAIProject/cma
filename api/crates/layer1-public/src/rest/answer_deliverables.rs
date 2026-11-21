use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::answer_deliverables::*;
use layer2_core::AgentId;
use std::sync::Arc;

#[path = "answer_deliverable_shapes.rs"]
mod shapes;
pub(crate) use shapes::*;

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(describe, list, read_item, download),
    components(schemas(
        DeliverableResourceView,
        DeliverableReferenceView,
        DeliverableDescriptionView,
        DeliverableStatusView,
        AnswerDeliverableView,
        AnswerDeliverablesView,
        AnswerDeliverableReadingView,
    ))
)]
pub(super) struct AnswerDeliverablesApi;

pub fn answer_deliverables_router(service: Arc<AnswerDeliverableService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/deliverable-references", post(describe))
        .route(
            "/api/v1/agents/{id}/answers/{answer_id}/deliverables",
            get(list),
        )
        .route(
            "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}",
            get(read_item),
        )
        .route(
            "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download",
            get(download),
        )
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}

async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = tokio::time::timeout(std::time::Duration::from_secs(30), next.run(request))
        .await
        .unwrap_or_else(|_| problem(DeliverableError::Unavailable));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}

fn prefers_native_media(headers: &HeaderMap) -> bool {
    headers.get_all("prefer").iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim() == "cma-native-attachment-media")
        })
    })
}

fn access<'a>(
    caller: &'a layer2_core::Caller,
    resource: &'a Option<AuthenticatedResourceCaller>,
    headers: &'a HeaderMap,
) -> Result<DeliverableAccess<'a>, super::documents::DocumentProblem> {
    Ok(DeliverableAccess {
        caller,
        context: resource.as_ref().map(|resource| &resource.context),
        credential: resource
            .as_ref()
            .and_then(|resource| resource.credential.as_ref())
            .map(DelegatedCredential::value),
        document_token: super::documents::share_token(headers)?,
    })
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/deliverable-references", tag = "agents",
    params(("id" = String, Path), ("X-CMA-Document-Share-Token" = Option<String>, Header),
        ("Prefer" = Option<String>, Header, description = "Use cma-native-attachment-media to request native media types")),
    request_body = DeliverableResourceView,
    responses((status = 200, body = DeliverableDescriptionView), (status = 401, body = super::ProblemDocument),
        (status = 404, body = super::ProblemDocument), (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn describe(
    State(service): State<Arc<AnswerDeliverableService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(agent): Path<String>,
    headers: HeaderMap,
    body: Result<Json<DeliverableResource>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<DeliverableDescription>, Response> {
    let body = body
        .map_err(|error| super::BodyProblem::from(error).into_response())?
        .0;
    let mut description = service
        .describe(
            &access(&caller, &resource, &headers).map_err(IntoResponse::into_response)?,
            &AgentId(agent),
            body,
        )
        .await
        .map_err(problem)?;
    if !prefers_native_media(&headers) {
        description.reference.use_legacy_file_media();
    }
    Ok(Json(description))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/answers/{answer_id}/deliverables", tag = "agents",
    params(("id" = String, Path), ("answer_id" = String, Path), ("X-CMA-Document-Share-Token" = Option<String>, Header),
        ("Prefer" = Option<String>, Header, description = "Use cma-native-attachment-media to request native media types")),
    responses((status = 200, body = AnswerDeliverablesView), (status = 401, body = super::ProblemDocument),
        (status = 404, body = super::ProblemDocument), (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Arc<AnswerDeliverableService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((agent, answer)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<AnswerDeliverables>, Response> {
    let mut list = service
        .list(
            &access(&caller, &resource, &headers).map_err(IntoResponse::into_response)?,
            &AgentId(agent),
            sequence(&answer).map_err(problem)?,
        )
        .await
        .map_err(problem)?;
    if !prefers_native_media(&headers) {
        for item in &mut list.items {
            item.reference.use_legacy_file_media();
        }
    }
    Ok(Json(list))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyParams {
    #[serde(default)]
    verify: bool,
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}", tag = "agents",
    params(("id" = String, Path), ("answer_id" = String, Path), ("delivery_id" = String, Path),
        ("verify" = Option<bool>, Query), ("X-CMA-Document-Share-Token" = Option<String>, Header),
        ("Prefer" = Option<String>, Header, description = "Use cma-native-attachment-media to request native media types")),
    responses((status = 200, body = AnswerDeliverableReadingView), (status = 401, body = super::ProblemDocument),
        (status = 404, body = super::ProblemDocument), (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn read_item(
    State(service): State<Arc<AnswerDeliverableService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((agent, answer, delivery)): Path<(String, String, String)>,
    query: Result<Query<VerifyParams>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Result<Json<AnswerDeliverableReading>, Response> {
    let query = query.map_err(|_| problem(DeliverableError::Invalid))?.0;
    let mut reading = service
        .read_item(
            &access(&caller, &resource, &headers).map_err(IntoResponse::into_response)?,
            &AgentId(agent),
            sequence(&answer).map_err(problem)?,
            &delivery,
            query.verify,
        )
        .await
        .map_err(problem)?;
    if !prefers_native_media(&headers) {
        reading.item.reference.use_legacy_file_media();
    }
    Ok(Json(reading))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download", tag = "agents",
    params(("id" = String, Path), ("answer_id" = String, Path), ("delivery_id" = String, Path), ("X-CMA-Document-Share-Token" = Option<String>, Header),
        ("Prefer" = Option<String>, Header, description = "Use cma-native-attachment-media to request native media types")),
    responses((status = 200, content(
        (Vec<u8> = "application/octet-stream"),
        (Vec<u8> = "text/csv"),
        (Vec<u8> = "application/json"),
        (Vec<u8> = "application/zip"),
        (Vec<u8> = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        (Vec<u8> = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"))),
        (status = 401, body = super::ProblemDocument), (status = 404, body = super::ProblemDocument),
        (status = 409, body = super::ProblemDocument), (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn download(
    State(service): State<Arc<AnswerDeliverableService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path((agent, answer, delivery)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let (reference, bytes) = service
        .download(
            &access(&caller, &resource, &headers).map_err(IntoResponse::into_response)?,
            &AgentId(agent),
            sequence(&answer).map_err(problem)?,
            &delivery,
        )
        .await
        .map_err(problem)?;
    let media_type = match reference.resource {
        DeliverableResource::LiveFile { .. } if prefers_native_media(&headers) => {
            reference.media_type
        }
        _ => "application/octet-stream".to_owned(),
    };
    Ok((
        [
            (header::CONTENT_TYPE, media_type),
            (
                header::CONTENT_DISPOSITION,
                super::agent_filesystem::download_name(&reference.name, &reference.sha256),
            ),
            (header::CONTENT_LENGTH, bytes.len().to_string()),
            (header::ETAG, format!("\"{}\"", reference.sha256)),
        ],
        bytes,
    )
        .into_response())
}

fn sequence(answer: &str) -> Result<u64, DeliverableError> {
    answer
        .strip_prefix("event:")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0 && answer == format!("event:{sequence}"))
        .ok_or(DeliverableError::Invalid)
}

fn problem(error: DeliverableError) -> Response {
    let (status, code) = match error {
        DeliverableError::Answer(error) => {
            return super::instance_problem::InstanceProblem::from(error).into_response()
        }
        DeliverableError::Artifact(error) => {
            return super::resource_access_problem::ResourceProblem(error).into_response()
        }
        DeliverableError::Invalid => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_deliverable_reference",
        ),
        DeliverableError::NotFound => (StatusCode::NOT_FOUND, "deliverable_not_found"),
        DeliverableError::Changed => (StatusCode::CONFLICT, "deliverable_changed"),
        DeliverableError::Unavailable => {
            (StatusCode::SERVICE_UNAVAILABLE, "deliverable_unavailable")
        }
        DeliverableError::Document(error) => {
            return super::documents::DocumentProblem::from(error).into_response()
        }
        DeliverableError::File(error) => {
            return super::agent_filesystem::FileProblem::from(error).into_response()
        }
    };
    super::problem_response(
        status,
        super::problem_document(status, code, &error.to_string(), code),
    )
}
