use std::{sync::Arc, time::Duration};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use layer2_core::{sandbox::store::CreationFence, Caller};

use super::*;

#[derive(Clone)]
pub struct AuthorizedLookup(pub SubjectId);

#[derive(Clone)]
pub struct Submission {
    pub deployment: String,
    pub wire_digest: String,
    pub fence: Option<CreationFence>,
    pub replay_operation_id: Option<String>,
}

pub enum Begin {
    Acquired(CreationFence),
    Existing {
        operation_id: Option<String>,
        cached_response: Option<Vec<u8>>,
    },
    Closed,
}

#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    fn deployment(&self) -> &str;

    async fn lookup(
        &self,
        identity: &Identity<'_>,
        query: &LookupRequest,
    ) -> Result<Outcome, StatusCode>;

    async fn begin(&self, identity: &Identity<'_>) -> Result<Begin, StatusCode>;

    async fn refuse_direct(
        &self,
        owner: &SubjectId,
        fence: &CreationFence,
        code: RefusalCode,
    ) -> Result<bool, StatusCode>;
}

pub fn router(backend: Arc<dyn Backend>) -> Router {
    Router::new()
        .route(LOOKUP_PATH, post(lookup))
        .layer(DefaultBodyLimit::max(MAX_LOOKUP_BYTES))
        .with_state(backend)
}

#[utoipa::path(
    post,
    path = "/api/v1/sandbox-creation-outcomes/lookup",
    tag = "sandboxes",
    operation_id = "lookupSandboxCreationOutcome",
    request_body = LookupRequest,
    responses(
        (status = 200, body = LookupResponse),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 401, body = crate::rest::ProblemDocument),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)
    )
)]
pub(in crate::rest) async fn lookup(
    State(backend): State<Arc<dyn Backend>>,
    authority: Option<Extension<AuthorizedLookup>>,
    request: Result<Json<LookupRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let mut response = match tokio::time::timeout(Duration::from_secs(2), async {
        let Extension(AuthorizedLookup(owner)) = authority.ok_or(StatusCode::UNAUTHORIZED)?;
        let Json(query) = request.map_err(|_| StatusCode::BAD_REQUEST)?;
        let identity = query
            .identity(&owner, backend.deployment())
            .map_err(identity_status)?;
        identity.period().map_err(identity_status)?;
        let result = backend.lookup(&identity, &query).await?;
        let response = LookupResponse {
            version: 1,
            deployment: identity.deployment.to_owned(),
            operation: identity.operation,
            identity_digest: identity.digest(),
            result,
        };
        let encoded = serde_json::to_vec(&response).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if encoded.len() > MAX_LOOKUP_BYTES {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(([(header::CONTENT_TYPE, "application/json")], encoded).into_response())
    })
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(status)) => problem(status),
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

pub(in crate::rest) fn identity_status(error: IdentityError) -> StatusCode {
    match error {
        IdentityError::Invalid => StatusCode::BAD_REQUEST,
        IdentityError::Mismatch => StatusCode::CONFLICT,
    }
}

pub fn problem(status: StatusCode) -> Response {
    let (code, text) = match status {
        StatusCode::BAD_REQUEST => (
            "invalid_creation_identity",
            "The creation identity is invalid.",
        ),
        StatusCode::UNAUTHORIZED => (
            "not_authenticated",
            "A verified owner identity is required.",
        ),
        StatusCode::FORBIDDEN => (
            "permission_denied",
            "This caller cannot inspect creation evidence.",
        ),
        StatusCode::CONFLICT => (
            "creation_identity_conflict",
            "This creation identity cannot be used.",
        ),
        _ => (
            "creation_outcome_unavailable",
            "Creation evidence is temporarily unavailable.",
        ),
    };
    crate::rest::problem_response(
        status,
        crate::rest::problem_document(status, code, text, code),
    )
}

pub(in crate::rest) async fn begin_submission(
    backend: Option<&dyn Backend>,
    caller: &Caller,
    operation: Operation,
    key: Option<&str>,
    digest: Option<&str>,
) -> Result<Option<Submission>, Response> {
    let Some(backend) = backend else {
        return Ok(None);
    };
    let Some(key) = key else { return Ok(None) };
    let Some(digest) = digest else {
        return Err(problem(StatusCode::BAD_REQUEST));
    };
    let versioned = is_covered_key(key);
    let (fence, replay_operation_id) = if versioned {
        let identity = Identity {
            owner: &caller.subject,
            deployment: backend.deployment(),
            operation,
            key,
            algorithm: Algorithm::NyxidWire,
            wire_digest: digest,
        };
        identity
            .period()
            .map_err(|error| problem(identity_status(error)))?;
        match backend.begin(&identity).await.map_err(problem)? {
            Begin::Acquired(fence) => (Some(fence), None),
            Begin::Existing {
                cached_response: Some(bytes),
                ..
            } => {
                return Err(crate::rest::idempotency::decode_response(&bytes)
                    .unwrap_or_else(|_| problem(StatusCode::SERVICE_UNAVAILABLE)));
            }
            Begin::Existing {
                operation_id: Some(id),
                ..
            } if operation == Operation::AgentBootstrap => (None, Some(id)),
            Begin::Existing { .. } | Begin::Closed => {
                return Err(problem(StatusCode::CONFLICT));
            }
        }
    } else {
        (None, None)
    };
    Ok(Some(Submission {
        deployment: backend.deployment().to_owned(),
        wire_digest: digest.to_owned(),
        fence,
        replay_operation_id,
    }))
}
