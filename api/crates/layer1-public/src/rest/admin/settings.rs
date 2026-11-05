//! Conditional replacement and presence-aware patching of platform settings.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{Capability, CoreError, PlatformSettings};

use crate::AuthenticatedCaller;

use super::{AdminState, NotAnOperator};
use crate::rest::{problem_document, problem_response};

mod body;

use body::SettingsJson;
pub(crate) use body::{
    InactivitySettingsBody, MarketplaceSettingsBody, PatchRequest, PlatformDefaultsBody,
    PlatformLimitsBody, PlatformSettingsBody, ProfileReferenceBody, ReplaceRequest, ResetRequest,
    RetentionSettingsBody, SettingsView, TriggerSettingsBody, UsageReportingSettingsBody,
};

pub(crate) enum Failure {
    Body(StatusCode, &'static str, &'static str, &'static str),
    NotAnOperator,
    Invalid(Option<(String, String)>),
    Conflict,
    Unavailable,
}

impl Failure {
    fn body(error: axum::extract::rejection::JsonRejection) -> Self {
        use axum::extract::rejection::JsonRejection;
        let status = error.status();
        match error {
            JsonRejection::JsonSyntaxError(_) => Self::Body(
                status,
                "malformed-json",
                "The request body is not valid JSON.",
                "malformed_json",
            ),
            JsonRejection::MissingJsonContentType(_) => Self::Body(
                status,
                "unsupported-media-type",
                "The request must use the application/json media type.",
                "unsupported_media_type",
            ),
            _ if status == StatusCode::PAYLOAD_TOO_LARGE => Self::Body(
                status,
                "request-too-large",
                "The request body exceeds what this service accepts.",
                "request_too_large",
            ),
            _ => Self::Invalid(None),
        }
    }
}

impl From<CoreError> for Failure {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Forbidden => Self::NotAnOperator,
            CoreError::InvalidRequest(_) | CoreError::InvalidPressurePolicy(_) => {
                Self::Invalid(None)
            }
            CoreError::InvalidState(_) => Self::Conflict,
            _ => Self::Unavailable,
        }
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        if matches!(self, Self::NotAnOperator) {
            return NotAnOperator.into_response();
        }
        let (status, slug, detail, code, field) = match self {
            Self::Body(status, slug, detail, code) => (status, slug, detail, code, None),
            Self::Invalid(field) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-request",
                "The JSON body does not match the platform settings contract.",
                "invalid_request",
                field,
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "settings-conflict",
                "Platform settings changed before this operation completed.",
                "settings_conflict",
                None,
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "settings-unavailable",
                "Platform settings could not be read or changed.",
                "settings_unavailable",
                None,
            ),
            Self::NotAnOperator => unreachable!("handled above"),
        };
        let mut body = problem_document(status, slug, detail, code);
        if let (Some((field, path)), Some(object)) = (field, body.as_object_mut()) {
            object.insert("field".into(), field.into());
            object.insert("path".into(), path.into());
        }
        problem_response(status, body)
    }
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route(
            "/api/v1/admin/settings",
            get(read).put(replace).patch(patch),
        )
        .route("/api/v1/admin/settings/reset", post(reset))
}

#[utoipa::path(get, path = "/api/v1/admin/settings", tag = "administration", responses((status = 200, body = SettingsView), (status = 403, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SettingsView>, Failure> {
    Ok(Json(state.settings.read(&caller).await?.into()))
}

#[utoipa::path(put, path = "/api/v1/admin/settings", tag = "administration", request_body = ReplaceRequest, responses((status = 200, body = SettingsView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn replace(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<ReplaceRequest>,
) -> Result<Json<SettingsView>, Failure> {
    Ok(Json(
        state
            .settings
            .configure_platform(&caller, body.expected_revision, &body.settings.into())
            .await?
            .into(),
    ))
}

#[utoipa::path(patch, path = "/api/v1/admin/settings", tag = "administration", request_body = PatchRequest, responses((status = 200, body = SettingsView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn patch(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<PatchRequest>,
) -> Result<Json<SettingsView>, Failure> {
    caller.require(Capability::ConfigurePlatform)?;
    let current = state.settings.current().await?;
    if current.revision != body.expected_revision {
        return Err(Failure::Conflict);
    }
    let (expected_revision, settings) = body.apply(current.settings)?;
    Ok(Json(
        state
            .settings
            .configure_platform(&caller, expected_revision, &settings)
            .await?
            .into(),
    ))
}

#[utoipa::path(post, path = "/api/v1/admin/settings/reset", tag = "administration", request_body = ResetRequest, responses((status = 200, body = SettingsView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn reset(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<ResetRequest>,
) -> Result<Json<SettingsView>, Failure> {
    Ok(Json(
        state
            .settings
            .configure_platform(
                &caller,
                body.expected_revision,
                &PlatformSettings::default(),
            )
            .await?
            .into(),
    ))
}
