//! Public Agent home file browser/editor API.
//!
//! All paths are relative to `/home/` in the runtime. Authorization is
//! revalidated by the layer-2 service before each operation, so revoked shares
//! stop working immediately.
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use base64::Engine as _;
use layer2_core::sandbox::agent_filesystem::{AgentFileError, AgentFileService, FileWriteTarget};
use layer2_core::AgentId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_AGENT_FILE_BYTES: u64 = 32 * 1024 * 1024;
#[path = "agent_browser.rs"]
pub(crate) mod browser;
#[path = "agent_browser_control.rs"]
pub(crate) mod browser_control;
#[path = "agent_environment.rs"]
pub(crate) mod environment;
use utoipa::ToSchema;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    max_bytes: Option<u64>,
    #[serde(default)]
    recursive: bool,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteFileRequest {
    path: String,
    #[serde(default)]
    expected_sha256: Option<String>,
    content_base64: String,
    #[serde(default)]
    directory: bool,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct FileEntryView {
    path: String,
    name: String,
    kind: String,
    size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    hidden: bool,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct FileListView {
    path: String,
    entries: Vec<FileEntryView>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct FileReadView {
    path: String,
    size_bytes: u64,
    sha256: String,
    content_base64: String,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct FileMutationView {
    path: String,
    size_bytes: Option<u64>,
    sha256: Option<String>,
    deleted: Option<bool>,
}

pub fn agent_filesystem_router(service: Arc<AgentFileService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/environment", get(environment::read))
        .route(
            "/api/v1/agents/{id}/browser/control",
            get(browser_control::read)
                .post(browser_control::change)
                .layer(axum::extract::DefaultBodyLimit::max(16384)),
        )
        .route(
            "/api/v1/agents/{id}/browser",
            get(browser::read)
                .put(browser::configure)
                .delete(browser::release)
                .layer(axum::extract::DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v1/agents/{id}/files",
            post(write)
                .delete(remove)
                .layer(axum::extract::DefaultBodyLimit::max(
                    (MAX_AGENT_FILE_BYTES.div_ceil(3) * 4 + 4096) as usize,
                )),
        )
        .route("/api/v1/agents/{id}/files/tree", get(list))
        .route(
            "/api/v1/agents/{id}/files/content",
            get(read).layer(axum::middleware::from_fn(
                super::dependency_diagnostics::file_content,
            )),
        )
        .route("/api/v1/agents/{id}/files/download", get(download))
        .layer(axum::middleware::from_fn(bounded_request))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}
async fn bounded_request(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    static CAPACITY: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let Ok(_permit) = CAPACITY
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .try_acquire()
    else {
        return super::problem_response(
            StatusCode::TOO_MANY_REQUESTS,
            super::problem_document(
                StatusCode::TOO_MANY_REQUESTS,
                "file_capacity",
                "File operations are busy. Retry shortly.",
                "file_capacity",
            ),
        );
    };
    match tokio::time::timeout(std::time::Duration::from_secs(60), next.run(request)).await {
        Ok(response) => response,
        Err(_) => super::problem_response(
            StatusCode::GATEWAY_TIMEOUT,
            super::problem_document(
                StatusCode::GATEWAY_TIMEOUT,
                "file_timeout",
                "File operation timed out. Refresh the file before retrying a save.",
                "file_timeout",
            ),
        ),
    }
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

#[utoipa::path(get, path = "/api/v1/agents/{id}/files/tree", tag = "agents",
    params(("id" = String, Path), ("path" = Option<String>, Query)),
    responses((status = 200, body = FileListView)))]
async fn list(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    query: Result<Query<FileQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<FileListView>, FileProblem> {
    let Query(query) = query.map_err(|_| FileProblem(AgentFileError::Invalid))?;
    let value = service
        .list(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            &query.path,
        )
        .await?;
    Ok(Json(FileListView {
        path: value.path,
        entries: value
            .entries
            .into_iter()
            .map(|e| FileEntryView {
                path: e.path,
                name: e.name,
                kind: format!("{:?}", e.kind).to_lowercase(),
                size_bytes: e.size_bytes,
                sha256: e.sha256,
                hidden: e.hidden,
            })
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/files/content", tag = "agents",
    params(("id" = String, Path), ("path" = String, Query), ("max_bytes" = Option<u64>, Query)),
    responses((status = 200, body = FileReadView)))]
async fn read(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    query: Result<Query<FileQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<FileReadView>, FileProblem> {
    let Query(query) = query.map_err(|_| FileProblem(AgentFileError::Invalid))?;
    let value = service
        .read(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            &query.path,
            query.max_bytes.unwrap_or(MAX_AGENT_FILE_BYTES),
        )
        .await?;
    Ok(Json(FileReadView {
        path: value.path,
        size_bytes: value.size_bytes,
        sha256: value.sha256,
        content_base64: value.content_base64,
    }))
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/files/download", tag = "agents",
    params(("id" = String, Path), ("path" = String, Query)),
    responses((status = 200, body = String, content_type = "application/octet-stream")))]
async fn download(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    query: Result<Query<FileQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, FileProblem> {
    let Query(query) = query.map_err(|_| FileProblem(AgentFileError::Invalid))?;
    let value = service
        .read(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            &query.path,
            MAX_AGENT_FILE_BYTES,
        )
        .await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value.content_base64)
        .map_err(|_| FileProblem(AgentFileError::Invalid))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                download_name(&value.path, &value.sha256),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/files", tag = "agents",
    params(("id" = String, Path)), request_body = WriteFileRequest,
    responses((status = 200, body = FileMutationView)))]
async fn write(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    target: Option<Extension<FileWriteTarget>>,
    Path(id): Path<String>,
    body: Result<Json<WriteFileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<FileMutationView>, FileProblem> {
    let Json(body) = body.map_err(|e| {
        FileProblem(if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AgentFileError::TooLarge
        } else {
            AgentFileError::Invalid
        })
    })?;
    let value = service
        .write(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            &body.path,
            body.expected_sha256,
            body.content_base64,
            body.directory,
            target.as_deref(),
        )
        .await?;
    Ok(Json(FileMutationView {
        path: value.path,
        size_bytes: Some(value.size_bytes),
        sha256: Some(value.sha256),
        deleted: None,
    }))
}

#[utoipa::path(delete, path = "/api/v1/agents/{id}/files", tag = "agents",
    params(("id" = String, Path), ("path" = String, Query), ("recursive" = Option<bool>, Query)),
    responses((status = 200, body = FileMutationView)))]
async fn remove(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    query: Result<Query<FileQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<FileMutationView>, FileProblem> {
    let Query(query) = query.map_err(|_| FileProblem(AgentFileError::Invalid))?;
    let value = service
        .delete(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            &query.path,
            query.recursive,
        )
        .await?;
    Ok(Json(FileMutationView {
        path: value.path,
        size_bytes: None,
        sha256: None,
        deleted: Some(value.deleted),
    }))
}

pub(super) fn download_name(path: &str, sha256: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("download");
    let encoded: String = name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"file-{sha256}\"; filename*=UTF-8''{encoded}")
}
fn context(
    resource: &Option<AuthenticatedResourceCaller>,
) -> Option<&layer2_core::resource_access::CallerContext> {
    resource.as_ref().map(|r| &r.context)
}
fn credential(resource: &Option<AuthenticatedResourceCaller>) -> Option<&str> {
    resource
        .as_ref()
        .and_then(|r| r.credential.as_ref())
        .map(DelegatedCredential::value)
}

pub(super) struct FileProblem(AgentFileError);
impl From<AgentFileError> for FileProblem {
    fn from(error: AgentFileError) -> Self {
        Self(error)
    }
}
impl IntoResponse for FileProblem {
    fn into_response(self) -> Response {
        if let AgentFileError::Access(error) = self.0 {
            return super::resource_access_problem::ResourceProblem(error).into_response();
        }
        let (status, code, detail) = match self.0 {
            AgentFileError::RuntimeCapacity(capacity) => {
                return super::problems::runtime_capacity(capacity)
            }
            AgentFileError::NotFound => (
                StatusCode::NOT_FOUND,
                "file_not_found",
                "File or directory does not exist.",
            ),
            AgentFileError::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
                "File exceeds the selected size limit.",
            ),
            AgentFileError::Invalid => (
                StatusCode::BAD_REQUEST,
                "invalid_file_request",
                "File path or content is invalid.",
            ),
            AgentFileError::NameTooLong => (
                StatusCode::BAD_REQUEST,
                "file_name_too_long",
                "File and folder names must be at most 255 UTF-8 bytes.",
            ),
            AgentFileError::NotReady => (
                StatusCode::CONFLICT,
                "file_runtime_not_ready",
                "The Agent runtime is not ready.",
            ),
            AgentFileError::Conflict => (
                StatusCode::CONFLICT,
                "file_conflict",
                "The file changed; refresh before saving.",
            ),
            AgentFileError::Unsupported => (
                StatusCode::CONFLICT,
                "file_runtime_unsupported",
                "This runtime does not expose its persistent home.",
            ),
            AgentFileError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "file_unavailable",
                "Filesystem access is temporarily unavailable.",
            ),
            AgentFileError::Access(_) => unreachable!(),
        };
        super::problem_response(status, super::problem_document(status, code, detail, code))
    }
}

#[cfg(test)]
#[path = "agent_filesystem_tests.rs"]
mod tests;
