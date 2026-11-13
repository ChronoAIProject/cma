//! Independent capacity configuration and bounded operator observations.
use super::AdminState;
use crate::AuthenticatedCaller;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::put,
    Json, Router,
};
use layer2_core::{admission::*, Capability, CoreError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};

impl AdminState {
    pub fn with_admission_ceilings(mut self, service: Arc<AdmissionCeilingsService>) -> Self {
        self.admission_ceilings = Some(service);
        self
    }
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new().route("/api/v1/admin/admission/ceilings", put(replace))
}

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ReserveView {
    retained_homes: Option<u32>,
    active_sessions: Option<u32>,
}

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CeilingsView {
    registrations: Option<u32>,
    retained_homes: Option<u32>,
    active_sessions: Option<u32>,
    owner_active_sessions: Option<u32>,
    existing_reserve: ReserveView,
}

impl From<AdmissionCeilings> for CeilingsView {
    fn from(value: AdmissionCeilings) -> Self {
        Self {
            registrations: value.registrations,
            retained_homes: value.retained_homes,
            active_sessions: value.active_sessions,
            owner_active_sessions: value.owner_active_sessions,
            existing_reserve: ReserveView {
                retained_homes: value.existing_reserve.retained_homes,
                active_sessions: value.existing_reserve.active_sessions,
            },
        }
    }
}
impl From<CeilingsView> for AdmissionCeilings {
    fn from(value: CeilingsView) -> Self {
        Self {
            registrations: value.registrations,
            retained_homes: value.retained_homes,
            active_sessions: value.active_sessions,
            owner_active_sessions: value.owner_active_sessions,
            existing_reserve: ExistingReserve {
                retained_homes: value.existing_reserve.retained_homes,
                active_sessions: value.existing_reserve.active_sessions,
            },
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct CeilingsChangeView {
    revision: u64,
    at: u64,
    actor: String,
    reason: String,
    old_ceilings: CeilingsView,
    new_ceilings: CeilingsView,
}
impl From<AdmissionCeilingsChange> for CeilingsChangeView {
    fn from(value: AdmissionCeilingsChange) -> Self {
        Self {
            revision: value.revision,
            at: value.at,
            actor: value.actor.to_string(),
            reason: value.reason,
            old_ceilings: value.old_ceilings.into(),
            new_ceilings: value.new_ceilings.into(),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct CeilingsStateView {
    revision: u64,
    ceilings: CeilingsView,
    last_change: Option<CeilingsChangeView>,
    writes_available: bool,
}
impl CeilingsStateView {
    fn new(value: AdmissionCeilingsState, writes_available: bool) -> Self {
        Self {
            revision: value.revision,
            ceilings: value.ceilings.into(),
            last_change: value.last_change.map(Into::into),
            writes_available,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct CapacityCountView {
    observed: u64,
    bound: u64,
    saturated: bool,
}
impl From<CapacityCount> for CapacityCountView {
    fn from(value: CapacityCount) -> Self {
        Self {
            observed: value.observed,
            bound: value.bound,
            saturated: value.saturated,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(super) struct CapacityView {
    registrations: Option<CapacityCountView>,
    retained_homes: Option<CapacityCountView>,
    pending_launch_homes: u64,
    expected_pending_launch_homes: u64,
    pending_launch_homes_drift: i64,
    active_sessions: Option<CapacityCountView>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct CeilingsReadView {
    ceilings: CeilingsStateView,
    effective_limits: CeilingsView,
    capacity: CapacityView,
    ceilings_history: Vec<CeilingsChangeView>,
}

pub(super) async fn read_view(
    state: &AdminState,
    caller: &layer2_core::Caller,
) -> Result<CeilingsReadView, CoreError> {
    let (read, writes) = match state.admission_ceilings.as_ref() {
        Some(service) => (
            service.read_with_history(caller).await?,
            service.writes_enabled(),
        ),
        None => (AdmissionCeilingsRead::default(), false),
    };
    Ok(CeilingsReadView {
        ceilings: CeilingsStateView::new(read.state, writes),
        effective_limits: read.effective_limits.into(),
        capacity: CapacityView {
            registrations: read.counts.registrations.map(Into::into),
            retained_homes: read.counts.retained_homes.map(Into::into),
            pending_launch_homes: read.counts.pending_launch_homes,
            expected_pending_launch_homes: read.counts.expected_pending_launch_homes,
            pending_launch_homes_drift: read.counts.pending_launch_homes_drift,
            active_sessions: read.counts.active_sessions.map(Into::into),
        },
        ceilings_history: read.history.into_iter().map(Into::into).collect(),
    })
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReplaceCeilingsRequest {
    ceilings: CeilingsView,
    reason: String,
    expected_revision: u64,
}

#[utoipa::path(put, path = "/api/v1/admin/admission/ceilings", tag = "administration",
    request_body = ReplaceCeilingsRequest, responses((status = 200, body = CeilingsStateView),
    (status = 403, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 422, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
async fn replace(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ReplaceCeilingsRequest>, JsonRejection>,
) -> Result<Json<CeilingsStateView>, Failure> {
    caller.require(Capability::ConfigurePlatform)?;
    let service = state
        .admission_ceilings
        .as_ref()
        .ok_or(CoreError::RuntimeUnavailable(
            "admission ceilings unavailable".into(),
        ))?;
    let Json(body) =
        body.map_err(|_| CoreError::InvalidRequest("invalid ceilings request".into()))?;
    let value = service
        .replace(
            &caller,
            body.expected_revision,
            &body.reason,
            body.ceilings.into(),
        )
        .await?;
    Ok(Json(CeilingsStateView::new(
        value,
        service.writes_enabled(),
    )))
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
            CoreError::Forbidden => return super::NotAnOperator.into_response(),
            CoreError::AdmissionCeilingsWritesUnavailable => (StatusCode::SERVICE_UNAVAILABLE,
                "admission-ceilings-writes-unavailable", "Saving admission ceilings is not available yet.", "admission_ceilings_writes_unavailable"),
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-admission-ceilings",
                "Supply valid ceilings, a bounded reason and the expected ceilings revision.", "invalid_request"),
            CoreError::InvalidState(_) => (StatusCode::CONFLICT, "admission-ceilings-conflict",
                "Admission ceilings changed. Read the current state before issuing a new command.", "admission_ceilings_conflict"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "admission-ceilings-unavailable",
                "Admission ceilings could not be read or changed. Read the current state before retrying an uncertain command.", "admission_ceilings_unavailable"),
        };
        crate::rest::problem_response(
            status,
            crate::rest::problem_document(status, slug, detail, code),
        )
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(replace),
    components(schemas(
        CeilingsView,
        ReserveView,
        CeilingsStateView,
        CeilingsChangeView,
        CapacityCountView,
        CapacityView,
        CeilingsReadView,
        ReplaceCeilingsRequest
    ))
)]
struct CeilingsApi;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    CeilingsApi::openapi()
}
