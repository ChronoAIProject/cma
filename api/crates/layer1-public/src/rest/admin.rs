//! Capability-gated REST projection of deployment operator controls.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::{OperatorError, SandboxOperatorService};
use layer2_core::{
    ActivityAction, ActivityQuery, ActivityRecord, ActivityService, ActivityTarget, Capability,
    CoreError, PlatformSettingsService, RuntimeInventory, RuntimeStatus, SubjectId,
    DEFAULT_ACTIVITY_LIMIT,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AuthenticatedCaller;

use super::{problem_document, problem_response};

pub(super) mod profiles;
pub(super) mod sandboxes;
pub(super) mod settings;
pub(super) mod triggers;
pub mod warm_pools;

/// Provider-neutral services used by the operator REST surface.
#[derive(Clone)]
pub struct AdminState {
    pub(super) warm_pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
    pub(super) users: Option<Arc<layer2_core::UserDirectory>>,
    pub(super) settings: Arc<PlatformSettingsService>,
    pub(super) sandboxes: Arc<SandboxOperatorService>,
    pub(super) activity: Arc<ActivityService>,
    pub(super) runtime_inventory: Option<Arc<dyn RuntimeInventory>>,
}

impl AdminState {
    pub fn new(
        settings: Arc<PlatformSettingsService>,
        sandboxes: Arc<SandboxOperatorService>,
        activity: Arc<ActivityService>,
    ) -> Self {
        Self {
            users: None,
            warm_pools: None,
            settings,
            sandboxes,
            activity,
            runtime_inventory: None,
        }
    }

    pub fn with_user_directory(mut self, users: Arc<layer2_core::UserDirectory>) -> Self {
        self.users = Some(users);
        self
    }

    pub fn with_warm_pools(mut self, service: Arc<dyn warm_pools::WarmPoolAdministration>) -> Self {
        self.warm_pools = Some(service);
        self
    }

    /// Supply the provider observation port when this deployment has one.
    pub fn with_runtime_inventory(mut self, inventory: Arc<dyn RuntimeInventory>) -> Self {
        self.runtime_inventory = Some(inventory);
        self
    }
}

/// Compose every route beneath the deliberately isolated admin path segment.
pub fn router(state: AdminState) -> Router {
    Router::new()
        .merge(settings::routes())
        .merge(sandboxes::routes())
        .merge(profiles::routes())
        .merge(triggers::routes())
        .merge(warm_pools::routes())
        .route("/api/v1/admin/runtime-inventory", get(runtime_inventory))
        .route("/api/v1/admin/activity", get(activity))
        .with_state(Arc::new(state))
}

pub(super) struct NotAnOperator;

impl IntoResponse for NotAnOperator {
    fn into_response(self) -> Response {
        let status = StatusCode::FORBIDDEN;
        problem_response(
            status,
            problem_document(
                status,
                "not-an-operator",
                "This operation requires an operator capability this caller does not hold.",
                "forbidden",
            ),
        )
    }
}

pub(crate) enum CollectionProblem {
    NotAnOperator,
    InvalidRequest,
    Conflict,
    NotFound,
    Unavailable,
}

impl From<CoreError> for CollectionProblem {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Forbidden => Self::NotAnOperator,
            CoreError::InvalidRequest(_) | CoreError::InvalidPressurePolicy(_) => {
                Self::InvalidRequest
            }
            CoreError::InvalidState(_) => Self::Conflict,
            CoreError::RuntimeUnavailable(_)
            | CoreError::RuntimeRefused(_)
            | CoreError::ServiceCatalogUnavailable(_)
            | CoreError::SkillCatalogUnavailable(_) => Self::Unavailable,
            CoreError::ServiceNotGranted(_) => Self::InvalidRequest,
        }
    }
}

impl From<OperatorError> for CollectionProblem {
    fn from(error: OperatorError) -> Self {
        match error {
            OperatorError::Forbidden => Self::NotAnOperator,
            OperatorError::NotFound => Self::NotFound,
            OperatorError::InvalidRequest(_) => Self::InvalidRequest,
            OperatorError::InvalidState(_) | OperatorError::ProtectedProfile => Self::Conflict,
            OperatorError::Unavailable(_) => Self::Unavailable,
        }
    }
}

impl IntoResponse for CollectionProblem {
    fn into_response(self) -> Response {
        if matches!(self, Self::NotAnOperator) {
            return NotAnOperator.into_response();
        }
        let (status, slug, detail, code) = match self {
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid-admin-request",
                "The operator request parameters are not valid.",
                "invalid_admin_request",
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "operator-conflict",
                "The operator action conflicts with current resource state.",
                "operator_conflict",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "operator-resource-not-found",
                "No operator resource matches this request.",
                "operator_resource_not_found",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "operator-service-unavailable",
                "An operator dependency could not complete this request.",
                "operator_service_unavailable",
            ),
            Self::NotAnOperator => unreachable!("handled above"),
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProtectedKind {
    Sandbox,
    Profile,
}

pub(crate) enum ProtectedProblem {
    Hidden(ProtectedKind),
    InvalidRequest,
    Conflict,
    ProtectedProfile,
    Unavailable,
}

impl ProtectedProblem {
    pub(super) fn sandbox(error: OperatorError) -> Self {
        Self::from_operator(error, ProtectedKind::Sandbox)
    }

    pub(super) fn profile(error: OperatorError) -> Self {
        Self::from_operator(error, ProtectedKind::Profile)
    }

    fn from_operator(error: OperatorError, kind: ProtectedKind) -> Self {
        match error {
            OperatorError::Forbidden | OperatorError::NotFound => Self::Hidden(kind),
            OperatorError::InvalidRequest(_) => Self::InvalidRequest,
            OperatorError::InvalidState(_) => Self::Conflict,
            OperatorError::ProtectedProfile => Self::ProtectedProfile,
            OperatorError::Unavailable(_) => Self::Unavailable,
        }
    }
}

impl IntoResponse for ProtectedProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self {
            Self::Hidden(ProtectedKind::Sandbox) => (
                StatusCode::NOT_FOUND,
                "sandbox-not-found",
                "No sandbox matches this request.",
                "sandbox_not_found",
            ),
            Self::Hidden(ProtectedKind::Profile) => (
                StatusCode::NOT_FOUND,
                "profile-not-found",
                "No profile matches this request.",
                "profile_not_found",
            ),
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "invalid-admin-request",
                "The operator request parameters are not valid.",
                "invalid_admin_request",
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "operator-conflict",
                "The operator action conflicts with current resource state.",
                "operator_conflict",
            ),
            Self::ProtectedProfile => (
                StatusCode::CONFLICT,
                "profile-protected",
                "Deployment-owned profiles cannot be retired.",
                "profile_protected",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "operator-service-unavailable",
                "An operator dependency could not complete this request.",
                "operator_service_unavailable",
            ),
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityParams {
    actor: Option<String>,
    action: Option<ActivityAction>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityView {
    id: String,
    at: String,
    actor: Option<String>,
    actor_name: Option<String>,
    target_name: Option<String>,
    #[schema(value_type = String)]
    action: ActivityAction,
    #[schema(value_type = Option<Object>)]
    target: Option<ActivityTarget>,
    #[schema(value_type = Object)]
    outcome: layer2_core::ActivityOutcome,
    #[schema(value_type = Object)]
    detail: layer2_core::ActivityDetail,
}

impl From<ActivityRecord> for ActivityView {
    fn from(record: ActivityRecord) -> Self {
        Self {
            id: record.id.0,
            at: record.at,
            actor: record.actor.map(|actor| actor.0),
            actor_name: None,
            target_name: None,
            action: record.action,
            target: record.target,
            outcome: record.outcome,
            detail: record.detail,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityPageView {
    items: Vec<ActivityView>,
    next: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/activity",
    tag = "administration",
    params(
        ("actor" = Option<String>, Query),
        ("action" = Option<String>, Query),
        ("limit" = Option<u32>, Query),
        ("cursor" = Option<String>, Query)
    ),
    responses(
        (status = 200, body = ActivityPageView),
        (status = 403, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn activity(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<ActivityParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ActivityPageView>, CollectionProblem> {
    let params = params.map_err(|_| CollectionProblem::InvalidRequest)?.0;
    let page = state
        .activity
        .list(
            &caller,
            &ActivityQuery {
                actor: params.actor.map(SubjectId),
                action: params.action,
                limit: params.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT),
                cursor: params.cursor,
            },
        )
        .await?;
    let mut subjects: Vec<_> = page
        .items
        .iter()
        .filter_map(|row| row.actor.clone())
        .collect();
    subjects.extend(page.items.iter().filter_map(|row| match &row.target {
        Some(ActivityTarget::Subject(subject)) => Some(subject.clone()),
        _ => None,
    }));
    subjects.sort_by(|a, b| a.0.cmp(&b.0));
    subjects.dedup();
    let mut names = std::collections::BTreeMap::new();
    for batch in subjects.chunks(200) {
        names.extend(super::public_names::resolve(&state.users, batch).await);
    }
    Ok(Json(ActivityPageView {
        items: page
            .items
            .into_iter()
            .map(|row| {
                let mut view = ActivityView::from(row);
                view.actor_name = view
                    .actor
                    .as_ref()
                    .and_then(|actor| names.get(actor))
                    .cloned();
                view.target_name = match &view.target {
                    Some(ActivityTarget::Subject(subject)) => names.get(&subject.0).cloned(),
                    _ => None,
                };
                view
            })
            .collect(),
        next: page.next_cursor,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RuntimeEntryView {
    runtime_id: String,
    generation: u64,
    owner_reference: Option<String>,
    status: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RuntimeInventoryView {
    observed_at: String,
    runtimes: Vec<RuntimeEntryView>,
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/runtime-inventory",
    tag = "administration",
    responses(
        (status = 200, body = RuntimeInventoryView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn runtime_inventory(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<RuntimeInventoryView>, CollectionProblem> {
    caller.require(Capability::ReadRuntimeInventory)?;
    let inventory = state
        .runtime_inventory
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?
        .snapshot()
        .await
        .map_err(|_| CollectionProblem::Unavailable)?;
    inventory
        .validate()
        .map_err(|_| CollectionProblem::Unavailable)?;
    Ok(Json(RuntimeInventoryView {
        observed_at: inventory.observed_at,
        runtimes: inventory
            .runtimes
            .into_iter()
            .map(|runtime| RuntimeEntryView {
                runtime_id: runtime.runtime_id,
                generation: runtime.generation,
                owner_reference: runtime.owner_reference,
                status: match runtime.status {
                    RuntimeStatus::Starting => "starting",
                    RuntimeStatus::Ready => "ready",
                    RuntimeStatus::Unavailable => "unavailable",
                }
                .into(),
            })
            .collect(),
    }))
}

#[cfg(test)]
#[path = "admin_tests.rs"]
pub(super) mod tests;
