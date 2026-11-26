//! Same-request identity condition for Public Entry command admission.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use layer2_core::Caller;

use super::{
    idempotency::{IdempotencyKey, RequestBody},
    problem_document, problem_response,
    trigger_openapi_shapes::{CreateLaunchRequest, LaunchView},
    triggers::TriggerRoutesState,
    BodyProblem,
};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller};

const EXPECTED: &str = "x-cma-expected-subject";
const VERIFIED: &str = "x-cma-verified-subject";
const MAX_ENCODED_BYTES: usize = 4096;

struct VerifiedSubject(HeaderValue);

enum SubjectConditionError {
    Invalid,
    Mismatch,
}

impl IntoResponse for SubjectConditionError {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self {
            Self::Invalid => (StatusCode::BAD_REQUEST, "command-subject-condition-invalid",
                "A single canonical expected command account is required.", "command_subject_condition_invalid"),
            Self::Mismatch => (StatusCode::CONFLICT, "command-subject-mismatch",
                "Sign in with the same CMA account that made this request. The original request remains unresolved.", "command_subject_mismatch"),
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

impl VerifiedSubject {
    fn check(headers: &HeaderMap, caller: &Caller) -> Result<Self, SubjectConditionError> {
        let mut values = headers.get_all(EXPECTED).iter();
        let value = values.next().ok_or(SubjectConditionError::Invalid)?;
        if values.next().is_some() || value.as_bytes().len() > MAX_ENCODED_BYTES {
            return Err(SubjectConditionError::Invalid);
        }
        let encoded = value
            .to_str()
            .map_err(|_| SubjectConditionError::Invalid)?
            .strip_prefix("v1.")
            .ok_or(SubjectConditionError::Invalid)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SubjectConditionError::Invalid)?;
        let subject = std::str::from_utf8(&bytes).map_err(|_| SubjectConditionError::Invalid)?;
        if subject.is_empty() || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
            return Err(SubjectConditionError::Invalid);
        }
        if subject != caller.subject.as_str() {
            return Err(SubjectConditionError::Mismatch);
        }
        let proof = format!("v1.{}", URL_SAFE_NO_PAD.encode(caller.subject.as_str()));
        Ok(Self(
            HeaderValue::from_str(&proof).map_err(|_| SubjectConditionError::Invalid)?,
        ))
    }

    fn stamp(self, mut response: Response) -> Response {
        response.headers_mut().insert(VERIFIED, self.0);
        response
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/triggers/public/{public_id}/launches",
    tag = "trigger launches",
    description = "Bind Launch admission or replay to this request's authenticated subject. The required condition is v1. followed by canonical unpadded base64url UTF-8, at most 4096 bytes. It never selects an authority. Uses the original v1 operation namespace, key and exact request bytes. A matched inner response, including replay or refusal, carries X-CMA-Verified-Subject; authentication, invalid conditions and subject mismatch do not. Clients must verify proof before consuming terminal results and must not fall back to v1.",
    params(
        ("public_id" = String, Path),
        ("Idempotency-Key" = String, Header),
        ("X-CMA-Expected-Subject" = String, Header, description = "Mandatory expected subject condition; canonical v1.base64url, at most 4096 bytes")
    ),
    request_body = CreateLaunchRequest,
    responses(
        (status = 201, body = LaunchView, headers(("X-CMA-Verified-Subject" = String))),
        (status = 202, body = LaunchView, headers(("X-CMA-Verified-Subject" = String))),
        (status = 400, body = super::ProblemDocument, description = "Missing or malformed subject condition; no admission"),
        (status = 401, body = super::ProblemDocument),
        (status = 403, body = super::ProblemDocument, headers(("X-CMA-Verified-Subject" = String))),
        (status = 409, body = super::ProblemDocument, description = "command_subject_mismatch has no proof or admission; matched domain refusals carry proof", headers(("X-CMA-Verified-Subject" = String))),
        (status = 422, body = super::ProblemDocument, headers(("X-CMA-Verified-Subject" = String))),
        (status = 429, body = super::ProblemDocument, headers(("X-CMA-Verified-Subject" = String))),
        (status = 503, body = super::ProblemDocument, headers(("X-CMA-Verified-Subject" = String)))
    )
)]
pub(super) async fn create_bound(
    state: State<TriggerRoutesState>,
    caller: AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    path: Path<String>,
    headers: HeaderMap,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateLaunchRequest>, BodyProblem>,
) -> Response {
    let verified = match VerifiedSubject::check(&headers, &caller.0) {
        Ok(verified) => verified,
        Err(problem) => return problem.into_response(),
    };
    verified.stamp(super::launches::create(state, caller, resource, path, key, body).await)
}
