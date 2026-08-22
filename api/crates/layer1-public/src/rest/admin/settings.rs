//! REST projection of deployment-wide settings.
//!
//! PATCH is conditional and setting-granular: omission leaves one setting
//! alone, while a supplied setting replaces that setting as a whole. `null`
//! has meaning only for the optional live-session limit. The briefing remains
//! an assembled type whose generated granted-services section is not request
//! data and therefore cannot be removed by a request.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{
    CapacityBounds, CoreError, PlatformSettings, PlatformSettingsService, SessionBriefing,
    StoredSettings,
};
use serde::{Deserialize, Deserializer, Serialize};

use super::AdminProblem;
use crate::auth::AuthenticatedCaller;
use crate::rest::ConfidentialBodyProblem;

/// The two exact paths owned by this module remain in the parent surface's
/// route list, so its invariant test still covers everything a gateway must
/// restrict.
pub(super) fn router(service: Arc<PlatformSettingsService>) -> Router {
    Router::new()
        .route(super::OPERATOR_ROUTES[6], get(read).patch(change))
        .route(super::OPERATOR_ROUTES[7], post(reset))
        .with_state(service)
}

/// The settings document together with the revision a future write must name.
#[derive(Debug, Serialize)]
struct SettingsView {
    revision: u64,
    settings: PlatformSettings,
    /// The full object replaced by the most recent write, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<PlatformSettings>,
}

impl From<StoredSettings> for SettingsView {
    fn from(stored: StoredSettings) -> Self {
        Self {
            revision: stored.revision,
            settings: stored.settings,
            previous: stored.previous,
        }
    }
}

/// A field on a settings PATCH.
///
/// Serde normally maps both a missing optional field and an explicit `null` to
/// `None`. Those are different acts here: missing means unchanged, while null
/// removes the live-session limit and is invalid for settings that are not
/// optional.
#[derive(Debug, Default)]
enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

/// All editable prose around the generated granted-services section.
///
/// Every field is required when the briefing setting is replaced. This keeps
/// a PATCH from silently filling an accidentally omitted field with shipped
/// prose, which would be neither "leave alone" nor what the client sent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BriefingRequest {
    before: Vec<String>,
    reach_heading: String,
    reach_lead: String,
    reach_close: String,
    reach_none: String,
    after: Vec<String>,
}

impl From<BriefingRequest> for SessionBriefing {
    fn from(value: BriefingRequest) -> Self {
        Self {
            before: value.before,
            reach_heading: value.reach_heading,
            reach_lead: value.reach_lead,
            reach_close: value.reach_close,
            reach_none: value.reach_none,
            after: value.after,
        }
    }
}

/// The complete range setting. Partial ranges are not meaningful: every
/// minimum must arrive with the maximum it is compared against.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundsRequest {
    min_cpu_millis: NonZeroU32,
    max_cpu_millis: NonZeroU32,
    min_memory_mib: NonZeroU32,
    max_memory_mib: NonZeroU32,
}

impl From<BoundsRequest> for CapacityBounds {
    fn from(value: BoundsRequest) -> Self {
        Self {
            min_cpu_millis: value.min_cpu_millis,
            max_cpu_millis: value.max_cpu_millis,
            min_memory_mib: value.min_memory_mib,
            max_memory_mib: value.max_memory_mib,
        }
    }
}

/// One conditional settings change.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    expected_revision: u64,
    #[serde(default)]
    briefing: PatchField<BriefingRequest>,
    #[serde(default)]
    size_bounds: PatchField<BoundsRequest>,
    #[serde(default)]
    live_sessions: PatchField<NonZeroU32>,
}

impl ChangeRequest {
    fn apply(self, mut settings: PlatformSettings) -> Result<(u64, PlatformSettings), CoreError> {
        if matches!(&self.briefing, PatchField::Missing)
            && matches!(&self.size_bounds, PatchField::Missing)
            && matches!(&self.live_sessions, PatchField::Missing)
        {
            return Err(CoreError::InvalidRequest(
                "a settings change must name at least one of 'briefing', 'size_bounds', or \
                 'live_sessions'"
                    .into(),
            ));
        }

        match self.briefing {
            PatchField::Missing => {}
            PatchField::Null => {
                return Err(CoreError::InvalidRequest(
                    "'briefing' cannot be null; use the reset operation to restore shipped prose"
                        .into(),
                ));
            }
            PatchField::Value(briefing) => settings.briefing = briefing.into(),
        }
        match self.size_bounds {
            PatchField::Missing => {}
            PatchField::Null => {
                return Err(CoreError::InvalidRequest(
                    "'size_bounds' cannot be null; send all four non-zero bounds".into(),
                ));
            }
            PatchField::Value(bounds) => settings.size_bounds = bounds.into(),
        }
        match self.live_sessions {
            PatchField::Missing => {}
            PatchField::Null => settings.live_sessions = None,
            PatchField::Value(limit) => settings.live_sessions = Some(limit),
        }
        Ok((self.expected_revision, settings))
    }
}

/// A conditional request to restore only the briefing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetRequest {
    expected_revision: u64,
}

/// A settings request can fail while being decoded or after reaching the core.
enum SettingsFailure {
    Body(ConfidentialBodyProblem),
    Refused(AdminProblem),
}

impl From<axum::extract::rejection::JsonRejection> for SettingsFailure {
    fn from(error: axum::extract::rejection::JsonRejection) -> Self {
        Self::Body(ConfidentialBodyProblem(error))
    }
}

impl From<CoreError> for SettingsFailure {
    fn from(error: CoreError) -> Self {
        Self::Refused(AdminProblem::from(error))
    }
}

impl IntoResponse for SettingsFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Body(problem) => problem.into_response(),
            Self::Refused(problem) => problem.into_response(),
        }
    }
}

async fn read(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SettingsView>, AdminProblem> {
    Ok(Json(service.read(&caller).await?.into()))
}

async fn change(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ChangeRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<SettingsView>, SettingsFailure> {
    let Json(body) = body?;
    let current = service.read(&caller).await?;
    let (expected, replacement) = body.apply(current.settings)?;
    Ok(Json(
        service.write(&caller, expected, replacement).await?.into(),
    ))
}

async fn reset(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ResetRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<SettingsView>, SettingsFailure> {
    let Json(body) = body?;
    Ok(Json(
        service
            .reset_briefing(&caller, body.expected_revision)
            .await?
            .into(),
    ))
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
