use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Router,
};
use layer2_core::{
    account_deletion::OwnerSessionAuthority,
    sandbox::{
        privacy_export::{PrivacyExport, PrivacyExportError},
        AgentReadError, AgentTranscriptPageRequest,
    },
    AgentId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AuthMethod, AuthenticatedCaller};

mod package;
#[cfg(test)]
pub(in crate::rest) mod tests;

const DEADLINE: Duration = Duration::from_secs(15);

pub fn router(service: Arc<PrivacyExport>) -> Router {
    Router::new()
        .route("/api/v1/account/privacy-export", get(inventory))
        .route(
            "/api/v1/account/privacy-export/agents/{id}/transcript",
            get(transcript),
        )
        .with_state(service)
        .layer(axum::middleware::map_response(private_response))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct InventoryQuery {
    cursor: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct Entry<'a> {
    id: &'a str,
    title: Option<&'a str>,
}

#[derive(Serialize, ToSchema)]
struct Inventory<'a> {
    owner_subject: &'a str,
    data_class: &'static str,
    agents: Vec<Entry<'a>>,
    next: Option<&'a str>,
}

#[utoipa::path(get, path = "/api/v1/account/privacy-export", tag = "privacy export",
    params(("cursor" = Option<String>, Query)),
    responses((status = 200, body = Inventory), (status = 400), (status = 401),
        (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn inventory(
    State(service): State<Arc<PrivacyExport>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
    authority: Option<Extension<OwnerSessionAuthority>>,
    query: Result<Query<InventoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, Problem> {
    let authority = owner_authority(method, authority.as_ref())?;
    let Query(query) = query.map_err(|_| Problem::Invalid)?;
    if query
        .cursor
        .as_deref()
        .is_some_and(|cursor| !valid_id(cursor))
    {
        return Err(Problem::Invalid);
    }
    within_deadline(async {
        let inventory = service
            .inventory(&caller, Some(authority), query.cursor)
            .await?;
        let body = encode(&Inventory {
            owner_subject: caller.subject.as_str(),
            data_class: "retained_conversation",
            agents: inventory
                .agents
                .items
                .iter()
                .map(|agent| Entry {
                    id: agent.id.as_str(),
                    title: agent.title.as_deref(),
                })
                .collect(),
            next: inventory.agents.next.as_deref(),
        })?;
        service.finish_inventory(&caller, inventory).await?;
        Ok(json_response(body))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TranscriptQuery {
    agent: Option<String>,
    turn: Option<u64>,
    after: Option<u64>,
    through: Option<u64>,
    limit: Option<u16>,
}

#[utoipa::path(get, path = "/api/v1/account/privacy-export/agents/{id}/transcript", tag = "privacy export",
    params(("id" = String, Path), ("agent" = Option<String>, Query), ("turn" = Option<u64>, Query),
        ("after" = Option<u64>, Query), ("through" = Option<u64>, Query),
        ("limit" = Option<u16>, Query, description = "Source entries per page, 1–32")),
    responses((status = 200, body = package::TranscriptPackage), (status = 400), (status = 401),
        (status = 403), (status = 404), (status = 409), (status = 413), (status = 503)))]
async fn transcript(
    State(service): State<Arc<PrivacyExport>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Option<Extension<AuthMethod>>,
    authority: Option<Extension<OwnerSessionAuthority>>,
    Path(id): Path<String>,
    query: Result<Query<TranscriptQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, Problem> {
    let authority = owner_authority(method, authority.as_ref())?;
    let Query(query) = query.map_err(|_| Problem::Invalid)?;
    if !valid_id(&id) || !valid_continuation(&id, &query) {
        return Err(Problem::Invalid);
    }
    let request = AgentTranscriptPageRequest {
        turn: query.turn,
        after: query.after,
        through: query.through,
        limit: query.limit.unwrap_or(32),
    };
    within_deadline(async {
        let reading = service
            .transcript(&caller, Some(authority), &AgentId(id), &request)
            .await?;
        let package =
            package::transcript_package(caller.subject.as_str(), &request, &reading.page)?;
        let body = encode(&package)?;
        service.finish_transcript(&caller, reading).await?;
        tracing::debug!(
            bytes = package.manifest.size_bytes,
            "Privacy export page prepared"
        );
        Ok(json_response(body))
    })
    .await
}

fn valid_continuation(id: &str, query: &TranscriptQuery) -> bool {
    if query.turn.is_none() && query.after.is_none() && query.through.is_none() {
        return query.agent.is_none();
    }
    query.agent.as_deref() == Some(id)
        && query.turn.is_some()
        && query.after.is_some()
        && query.through.is_some()
        && query.limit.is_some()
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn owner_authority(
    method: Option<Extension<AuthMethod>>,
    authority: Option<&Extension<OwnerSessionAuthority>>,
) -> Result<&OwnerSessionAuthority, Problem> {
    match (method, authority) {
        (None, Some(Extension(authority))) => Ok(authority),
        _ => Err(Problem::Domain(PrivacyExportError::OwnerSessionRequired)),
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, Problem> {
    serde_json::to_vec(value).map_err(|_| Problem::Unavailable)
}

fn json_response(body: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn within_deadline(
    operation: impl std::future::Future<Output = Result<Response, Problem>>,
) -> Result<Response, Problem> {
    tokio::time::timeout(DEADLINE, operation)
        .await
        .map_err(|_| Problem::Unavailable)?
}

async fn private_response(mut response: Response) -> Response {
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

enum Problem {
    Domain(PrivacyExportError),
    Invalid,
    Unavailable,
    TooLarge,
}

impl From<PrivacyExportError> for Problem {
    fn from(error: PrivacyExportError) -> Self {
        Self::Domain(error)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        use PrivacyExportError::*;
        let (status, code, detail) = match self {
            Self::Invalid | Self::Domain(Read(AgentReadError::InvalidPage)) => (
                StatusCode::BAD_REQUEST,
                "privacy_export_invalid_page",
                "The export page is invalid.",
            ),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "privacy_export_page_too_large",
                "The export page exceeds its delivery limit. Try a smaller page.",
            ),
            Self::Domain(OwnerSessionRequired) => (
                StatusCode::FORBIDDEN,
                "privacy_export_owner_session_required",
                "Sign in as the personal owner to export account data.",
            ),
            Self::Domain(Erasing) => (
                StatusCode::FORBIDDEN,
                "account_deletion_in_progress",
                "Account deletion is in progress.",
            ),
            Self::Domain(SourceChanged) => (
                StatusCode::CONFLICT,
                "privacy_export_source_changed",
                "The export source changed. Start a fresh export.",
            ),
            Self::Domain(SourceUnavailable) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "privacy_export_source_unavailable",
                "This history cannot currently be exported within the source limits.",
            ),
            Self::Domain(Read(error)) => {
                return super::instance_problem::InstanceProblem::from(error).into_response()
            }
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "privacy_export_unavailable",
                "The export could not be prepared.",
            ),
        };
        tracing::debug!(code, "Privacy export request refused");
        super::problem_response(
            status,
            super::problem_document(status, "privacy-export", detail, code),
        )
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(inventory, transcript))]
struct PrivacyExportApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    PrivacyExportApi::openapi()
}
