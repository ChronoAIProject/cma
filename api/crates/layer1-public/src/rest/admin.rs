//! Capability-gated REST projection of deployment operator controls.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::sandbox::{OperatorError, SandboxMaintenanceStore, SandboxOperatorService};
use layer2_core::{
    ActivityService, Capability, CoreError, PlatformSettingsService, RuntimeInventory,
    RuntimeStatus, SubjectId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AuthenticatedCaller;

use super::{problem_document, problem_response};

mod activity;
pub(super) mod binding_holds;
pub(crate) use activity::{
    __path_activity, __path_activity_actors, activity, activity_actors, ActivityActorView,
    ActivityPageView, ActivityView,
};

pub(super) mod admission;
pub(super) mod auto_pause;
pub(super) mod ceilings;
mod pending_homes;
pub(super) mod profiles;
pub(super) mod runtime_migration;
pub(super) mod runtime_update;
pub(super) mod sandboxes;
pub(super) mod settings;
pub(super) mod spend;
pub(super) mod triggers;
pub mod warm_pools;

/// Provider-neutral services used by the operator REST surface.
#[derive(Clone)]
pub struct AdminState {
    pub(super) binding_diagnostics:
        Option<Arc<layer2_core::binding_diagnostics::BindingDiagnosticsService>>,
    pub(super) warm_pools: Option<Arc<dyn warm_pools::WarmPoolAdministration>>,
    pub(super) users: Option<Arc<layer2_core::UserDirectory>>,
    pub(super) settings: Arc<PlatformSettingsService>,
    pub(super) sandboxes: Arc<SandboxOperatorService>,
    pub(super) activity: Arc<ActivityService>,
    pub(super) runtime_inventory: Option<Arc<dyn RuntimeInventory>>,
    pub(super) runtime_migrations:
        Option<Arc<layer2_core::sandbox::SandboxCustomRuntimeMigrationService>>,
    pub(super) runtime_activity: Option<Arc<layer2_core::sandbox::RuntimeActivityVerifier>>,
    pub(super) runtime_updates: Option<Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>>,
    pub(super) auto_pause: Option<Arc<layer2_core::sandbox::SandboxAutoPauseService>>,
    pub(super) spend: Option<Arc<layer2_core::budget::PlatformSpendService>>,
    pub(super) admission_ceilings: Option<Arc<layer2_core::admission::AdmissionCeilingsService>>,
    pub(super) admission_control: Option<Arc<layer2_core::admission::AdmissionControlService>>,
}

impl AdminState {
    pub fn new(
        settings: Arc<PlatformSettingsService>,
        sandboxes: Arc<SandboxOperatorService>,
        activity: Arc<ActivityService>,
    ) -> Self {
        Self {
            binding_diagnostics: None,
            users: None,
            warm_pools: None,
            settings,
            sandboxes,
            activity,
            runtime_inventory: None,
            runtime_updates: None,
            auto_pause: None,
            runtime_migrations: None,
            runtime_activity: None,
            spend: None,
            admission_control: None,
            admission_ceilings: None,
        }
    }

    pub fn with_user_directory(mut self, users: Arc<layer2_core::UserDirectory>) -> Self {
        self.users = Some(users);
        self
    }

    pub fn with_binding_diagnostics(
        mut self,
        service: Arc<layer2_core::binding_diagnostics::BindingDiagnosticsService>,
    ) -> Self {
        self.binding_diagnostics = Some(service);
        self
    }

    pub fn with_warm_pools(mut self, service: Arc<dyn warm_pools::WarmPoolAdministration>) -> Self {
        self.warm_pools = Some(service);
        self
    }

    pub fn with_runtime_migrations(
        mut self,
        service: Arc<layer2_core::sandbox::SandboxCustomRuntimeMigrationService>,
        verifier: Option<Arc<layer2_core::sandbox::RuntimeActivityVerifier>>,
    ) -> Self {
        self.runtime_migrations = Some(service);
        self.runtime_activity = verifier;
        self
    }

    pub fn with_runtime_updates(
        mut self,
        service: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    ) -> Self {
        self.runtime_updates = Some(service);
        self
    }

    /// Supply the provider observation port when this deployment has one.
    pub fn with_runtime_inventory(mut self, inventory: Arc<dyn RuntimeInventory>) -> Self {
        self.runtime_inventory = Some(inventory);
        self
    }

    pub fn with_maintenance(mut self, maintenance: Arc<dyn SandboxMaintenanceStore>) -> Self {
        self.auto_pause = Some(Arc::new(
            layer2_core::sandbox::SandboxAutoPauseService::new(
                self.settings.clone(),
                maintenance,
                self.activity.clone(),
            ),
        ));
        self
    }

    pub fn with_platform_spend(
        mut self,
        service: Arc<layer2_core::budget::PlatformSpendService>,
    ) -> Self {
        self.spend = Some(service);
        self
    }
}

/// Compose every route beneath the deliberately isolated admin path segment.
pub fn router(state: AdminState) -> Router {
    let diagnostics = if state.binding_diagnostics.is_some() {
        Router::new().route(
            "/api/v1/admin/runtime-binding-holds",
            get(binding_holds::read),
        )
    } else {
        Router::new()
    };
    Router::new()
        .merge(diagnostics)
        .merge(settings::routes(
            state.settings.clone(),
            state.users.clone(),
        ))
        .merge(spend::routes())
        .merge(admission::routes())
        .merge(auto_pause::routes())
        .merge(sandboxes::routes())
        .merge(runtime_update::routes())
        .merge(runtime_migration::routes())
        .merge(profiles::routes())
        .merge(triggers::routes())
        .merge(warm_pools::routes())
        .route("/api/v1/admin/runtime-inventory", get(runtime_inventory))
        .route("/api/v1/admin/activity", get(activity))
        .route("/api/v1/admin/activity/actors", get(activity_actors))
        .with_state(Arc::new(state))
}

/// Platform settings with their usual caller, revision and write checks.
pub fn platform_settings_router(settings: Arc<layer2_core::PlatformSettingsService>) -> Router {
    settings::routes(settings, None)
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
    AdmissionPaused,
    AdmissionWritesUnavailable,
    NotAnOperator,
    InvalidRequest,
    Conflict,
    NotFound,
    Unavailable,
}

impl From<CoreError> for CollectionProblem {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::AdmissionPaused => Self::AdmissionPaused,
            CoreError::AdmissionControlWritesUnavailable => Self::AdmissionWritesUnavailable,
            CoreError::Forbidden => Self::NotAnOperator,
            CoreError::InvalidRequest(_) | CoreError::InvalidPressurePolicy(_) => {
                Self::InvalidRequest
            }
            CoreError::InvalidState(_) => Self::Conflict,
            CoreError::AdmissionCeilingsWritesUnavailable
            | CoreError::AdmissionCapacity(_)
            | CoreError::ModelBudgetWritesUnavailable
            | CoreError::RuntimeUnavailable(_)
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
            Self::AdmissionPaused => return super::admission_problem::paused(),
            Self::AdmissionWritesUnavailable => {
                return super::admission_problem::writes_unavailable()
            }
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
