//! Capability-gated operator admission control, independent of spend settings.

use std::sync::Arc;

use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::admission::{
    AdmissionChange, AdmissionControl, AdmissionControlService, OperatorStop,
};
use layer2_core::{Capability, CoreError};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use super::{AdminState, NotAnOperator};
use crate::AuthenticatedCaller;

impl AdminState {
    pub fn with_admission_control(mut self, service: Arc<AdmissionControlService>) -> Self {
        self.admission_control = Some(service);
        self
    }
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/admission", get(read))
        .route("/api/v1/admin/admission/stop", post(stop))
        .route("/api/v1/admin/admission/resume", post(resume))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    /// Nonblank text, at most 512 UTF-8 bytes, with no control characters.
    reason: String,
    /// Independent admission revision; resume must name the active stop.
    expected_revision: u64,
}

#[derive(Serialize, ToSchema)]
struct StopView {
    /// Unix seconds at which this stop was committed.
    since: u64,
    revision: u64,
    actor: String,
    reason: String,
}

impl From<OperatorStop> for StopView {
    fn from(value: OperatorStop) -> Self {
        Self {
            since: value.since,
            revision: value.revision,
            actor: value.actor.to_string(),
            reason: value.reason,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ChangeView {
    revision: u64,
    /// Unix seconds of the control change, shared with its activity record.
    at: u64,
    actor: String,
    reason: String,
    old_stop: Option<StopView>,
    new_stop: Option<StopView>,
}

impl From<AdmissionChange> for ChangeView {
    fn from(value: AdmissionChange) -> Self {
        Self {
            revision: value.revision,
            at: value.at,
            actor: value.actor.to_string(),
            reason: value.reason,
            old_stop: value.old_stop.map(Into::into),
            new_stop: value.new_stop.map(Into::into),
        }
    }
}

#[derive(Serialize, ToSchema)]
struct AdmissionView {
    revision: u64,
    operator_stop: Option<StopView>,
    last_change: Option<ChangeView>,
    /// Independent deployment write gate; enforcement is always enabled.
    writes_available: bool,
}

impl AdmissionView {
    fn new(control: AdmissionControl, service: &AdmissionControlService) -> Self {
        Self {
            revision: control.revision,
            operator_stop: control.operator_stop.map(Into::into),
            last_change: control.last_change.map(Into::into),
            writes_available: service.writes_enabled(),
        }
    }
}

#[derive(Serialize, ToSchema)]
struct AdmissionReadView {
    #[serde(flatten)]
    control: AdmissionView,
    /// At most twenty immutable changes, newest revision first.
    history: Vec<ChangeView>,
}

#[utoipa::path(get, path = "/api/v1/admin/admission", tag = "administration", responses(
    (status = 200, body = AdmissionReadView), (status = 403, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)
))]
async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<AdmissionReadView>, Failure> {
    caller.require(Capability::ConfigurePlatform)?;
    let service =
        state
            .admission_control
            .as_ref()
            .ok_or(Failure(CoreError::RuntimeUnavailable(
                "admission control is unavailable".into(),
            )))?;
    let read = service.read_with_history(&caller).await?;
    Ok(Json(AdmissionReadView {
        control: AdmissionView::new(read.control, service),
        history: read.history.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/admin/admission/stop", tag = "administration", request_body = ChangeRequest, responses(
    (status = 200, body = AdmissionView), (status = 403, body = crate::rest::ProblemDocument),
    (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)
))]
async fn stop(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ChangeRequest>, JsonRejection>,
) -> Result<Json<AdmissionView>, Failure> {
    change(&state, &caller, body, true).await
}

#[utoipa::path(post, path = "/api/v1/admin/admission/resume", tag = "administration", request_body = ChangeRequest, responses(
    (status = 200, body = AdmissionView), (status = 403, body = crate::rest::ProblemDocument),
    (status = 409, body = crate::rest::ProblemDocument), (status = 422, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)
))]
async fn resume(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ChangeRequest>, JsonRejection>,
) -> Result<Json<AdmissionView>, Failure> {
    change(&state, &caller, body, false).await
}

async fn change(
    state: &AdminState,
    caller: &layer2_core::Caller,
    body: Result<Json<ChangeRequest>, JsonRejection>,
    stopped: bool,
) -> Result<Json<AdmissionView>, Failure> {
    caller.require(Capability::ConfigurePlatform)?;
    let service =
        state
            .admission_control
            .as_ref()
            .ok_or(Failure(CoreError::RuntimeUnavailable(
                "admission control is unavailable".into(),
            )))?;
    let Json(body) = body.map_err(|_| {
        Failure(CoreError::InvalidRequest(
            "invalid admission control request".into(),
        ))
    })?;
    let control = if stopped {
        service
            .stop(caller, body.expected_revision, &body.reason)
            .await?
    } else {
        service
            .resume(caller, body.expected_revision, &body.reason)
            .await?
    };
    Ok(Json(AdmissionView::new(control, service)))
}

struct Failure(CoreError);
impl From<CoreError> for Failure {
    fn from(value: CoreError) -> Self {
        Self(value)
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self.0 {
            CoreError::Forbidden => return NotAnOperator.into_response(),
            CoreError::AdmissionControlWritesUnavailable => return crate::rest::admission_problem::writes_unavailable(),
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-admission-control", "Supply a bounded reason and the expected admission revision.", "invalid_request"),
            CoreError::InvalidState(_) => (StatusCode::CONFLICT, "admission-control-conflict", "Admission control changed. Read the current state before issuing a new command.", "admission_control_conflict"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "admission-control-unavailable", "Admission control could not be read or changed. Read the current state before retrying an uncertain command.", "admission_control_unavailable"),
        };
        crate::rest::problem_response(
            status,
            crate::rest::problem_document(status, slug, detail, code),
        )
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(read, stop, resume),
    components(schemas(AdmissionView, AdmissionReadView, StopView, ChangeView, ChangeRequest))
)]
struct AdmissionApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    AdmissionApi::openapi()
}
