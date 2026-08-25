//! REST projection of the operator surface.
//!
//! Operator reads and controls, all under one path segment so a gateway can
//! restrict the whole surface as a unit and so an operator acting on somebody
//! else's work is doing it at a deliberately different address from the one
//! they use on their own.
//!
//! Every handler is a thin translation onto one core call. Which capability
//! each needs is decided in Layer 2 and checked there; nothing here holds an
//! opinion about who may see what, and the two-capability endpoints simply
//! make two core calls and let the first refusal answer.
//!
//! Refusals are rendered differently here than on the session surface, and the
//! difference is deliberate. A session is addressed by an identifier, so a
//! caller who may not see one is told it does not exist — confirming the
//! identifier would itself be a disclosure. These collections are addressed by
//! nothing: there is no identifier to protect, so a caller who is not an
//! operator is told plainly that they are not one.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json, Router,
};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, Caller, CoreError, DirectoryQuery,
    PlatformSettingsService, ProviderView, RegisteredUser, RuntimeInventory, Session, SessionId,
    SessionService, SubjectId, UserDirectory, DEFAULT_ACTIVITY_LIMIT, DEFAULT_DIRECTORY_LIMIT,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::auth::{ActedUpon, AuthenticatedCaller};
use crate::rest::openapi_shapes;
use crate::rest::pagination::AdminSessionPageParams;
use crate::rest::{problem_document, problem_response, Problem, QueryProblem};

mod settings;

/// What the operator surface reads from.
///
/// Three services rather than one god object: each answers for its own
/// capability, and a deployment could replace any of them without the other
/// two knowing.
pub struct AdminState {
    /// Sessions, and the runtimes serving them.
    pub sessions: Arc<SessionService>,
    /// The record of what the service was asked to do.
    pub activity: Arc<ActivityService>,
    /// Who has used the service.
    pub users: Arc<UserDirectory>,
    /// Deployment-wide policy read and changed by administrators.
    pub settings: Arc<PlatformSettingsService>,
}

/// A refusal that names no resource.
///
/// Distinct from [`Problem`], which answers "not found" to protect an
/// identifier. Nothing is being protected here, so saying so plainly is both
/// more honest and more useful to whoever is holding the wrong role.
struct NotAnOperator;

impl IntoResponse for NotAnOperator {
    fn into_response(self) -> Response {
        let status = StatusCode::FORBIDDEN;
        let body = problem_document(
            status,
            "not-an-operator",
            "This operation requires an operator capability this caller does not hold.",
            "forbidden",
        );
        problem_response(status, body)
    }
}

/// A refusal on an operator collection.
///
/// Anything that is not a bare lack of entitlement keeps the ordinary problem
/// rendering, so an unreachable store still reads as unavailability rather
/// than being flattened into "you may not".
enum AdminProblem {
    /// The caller holds no operator capability.
    NotAnOperator,
    /// Something else went wrong, rendered as it is everywhere else.
    Other(Problem),
}

impl From<CoreError> for AdminProblem {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::Forbidden => Self::NotAnOperator,
            other => Self::Other(Problem(other)),
        }
    }
}

impl IntoResponse for AdminProblem {
    fn into_response(self) -> Response {
        match self {
            Self::NotAnOperator => NotAnOperator.into_response(),
            Self::Other(problem) => problem.into_response(),
        }
    }
}

/// An operator request can fail before or after its query is decoded.
enum AdminRequestFailure {
    Query(QueryProblem),
    Refused(AdminProblem),
}

impl From<axum::extract::rejection::QueryRejection> for AdminRequestFailure {
    fn from(error: axum::extract::rejection::QueryRejection) -> Self {
        Self::Query(QueryProblem(error))
    }
}

impl From<CoreError> for AdminRequestFailure {
    fn from(error: CoreError) -> Self {
        Self::Refused(AdminProblem::from(error))
    }
}

impl IntoResponse for AdminRequestFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Query(problem) => problem.into_response(),
            Self::Refused(problem) => problem.into_response(),
        }
    }
}

/// Somebody who has used the service, as this interface renders them.
///
/// The identifier is present because clients key on it, and it is deliberately
/// not what a person is shown by: `label` is what a screen draws, falling back
/// to the identifier only when the provider released nothing at all.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserView {
    subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    first_seen: String,
    last_seen: String,
}

impl From<RegisteredUser> for UserView {
    fn from(u: RegisteredUser) -> Self {
        Self {
            subject: u.subject.0.clone(),
            label: u.label().map(str::to_string),
            display_name: u.display_name,
            email: u.email,
            first_seen: u.first_seen,
            last_seen: u.last_seen,
        }
    }
}

/// One recorded action, as this interface renders it.
///
/// The core's own vocabulary is emitted directly rather than remapped: these
/// strings are the record, and translating them here would mean two spellings
/// of the same fact.
#[derive(Debug, Serialize, ToSchema)]
pub struct ActivityView {
    id: String,
    at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[schema(value_type = openapi_shapes::ActivityAction)]
    action: layer2_core::ActivityAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<openapi_shapes::ActivityTarget>)]
    target: Option<layer2_core::ActivityTarget>,
    #[schema(value_type = openapi_shapes::ActivityOutcome)]
    outcome: layer2_core::ActivityOutcome,
    /// The parameters that were allowed to be kept. Never a secret: the core's
    /// allowlist is what decides, and there is no key for one.
    #[schema(value_type = std::collections::BTreeMap<String, String>)]
    detail: layer2_core::ActivityDetail,
}

impl From<ActivityRecord> for ActivityView {
    fn from(r: ActivityRecord) -> Self {
        Self {
            id: r.id.0,
            at: r.at,
            actor: r.actor.map(|s| s.0),
            action: r.action,
            target: r.target,
            outcome: r.outcome,
            detail: r.detail,
        }
    }
}

/// A session as the operator surface renders it.
///
/// Carries its owner, which the ordinary session view does not: a list of
/// everybody's work is unreadable without saying whose each row is.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminSessionView {
    id: String,
    owner: String,
    /// How the owner should be shown, when the directory knows a name for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_label: Option<String>,
    #[schema(value_type = openapi_shapes::SessionStatus)]
    status: layer2_core::SessionStatus,
    #[schema(value_type = openapi_shapes::RecoveryState)]
    recovery_state: layer2_core::RecoveryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<openapi_shapes::StopReason>)]
    stop_reason: Option<layer2_core::StopReason>,
    kernel: String,
    last_seq: u64,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_version: Option<String>,
}

impl AdminSessionView {
    fn render(s: Session, names: &HashMap<SubjectId, String>) -> Self {
        Self {
            owner_label: names.get(&s.owner).cloned(),
            owner: s.owner.0,
            id: s.id.0,
            status: s.status,
            recovery_state: s.recovery_state,
            stop_reason: s.stop_reason,
            kernel: s.kernel.0,
            last_seq: s.last_seq,
            created_at: s.created_at,
            runtime_version: s.runtime_version,
        }
    }
}

/// One runtime in service, as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
pub struct RuntimeView {
    runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<openapi_shapes::SessionStatus>)]
    status: Option<layer2_core::SessionStatus>,
    /// Whether nothing in the record accounts for this runtime.
    unaccounted: bool,
}

/// What one provider had to say, as this interface renders it.
///
/// `observable` is false when the provider could not be asked, and `runtimes`
/// is then absent rather than empty. A client cannot accidentally read "we
/// could not ask" as "there is nothing running", because there is no empty
/// array there to read.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderInventoryView {
    kernel: String,
    observable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtimes: Option<Vec<RuntimeView>>,
}

/// Everything in service, as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
pub struct InventoryView {
    /// How many runtimes are in service. Absent when a provider could not be
    /// asked, because a partial total presented as a total is the same lie as
    /// an empty list.
    #[serde(skip_serializing_if = "Option::is_none")]
    running: Option<usize>,
    /// Whether every provider answered.
    fully_observed: bool,
    providers: Vec<ProviderInventoryView>,
}

impl InventoryView {
    fn render(inventory: RuntimeInventory) -> Self {
        Self {
            running: inventory.running(),
            fully_observed: inventory.fully_observed(),
            providers: inventory
                .providers
                .into_iter()
                .map(|p| match p.view {
                    ProviderView::Observed(runtimes) => ProviderInventoryView {
                        kernel: p.kernel.0,
                        observable: true,
                        reason: None,
                        runtimes: Some(
                            runtimes
                                .into_iter()
                                .map(|r| RuntimeView {
                                    unaccounted: r.is_unaccounted(),
                                    runtime: r.runtime,
                                    session: r.session.map(|s| s.0),
                                    owner: r.owner.map(|o| o.0),
                                    status: r.status,
                                })
                                .collect(),
                        ),
                    },
                    ProviderView::Unobservable(reason) => ProviderInventoryView {
                        kernel: p.kernel.0,
                        observable: false,
                        reason: Some(reason),
                        runtimes: None,
                    },
                })
                .collect(),
        }
    }
}

/// Which entries a read of the directory wants.
#[derive(Debug, Deserialize)]
pub struct DirectoryParams {
    /// Most entries to return. The core clamps this, as it does for the record.
    #[serde(default)]
    limit: Option<usize>,
}

/// Which entries a read of the record wants.
#[derive(Debug, Deserialize)]
pub struct ActivityParams {
    /// Narrow to one subject.
    #[serde(default)]
    actor: Option<String>,
    /// Most entries to return. The core clamps this; a client asking for more
    /// than the log will serve gets as much as it will.
    #[serde(default)]
    limit: Option<usize>,
}

/// Every path this surface serves.
///
/// Named once so the router and the rule about them cannot drift: the whole
/// surface lives under one segment precisely so a gateway can restrict it as a
/// unit, and a route added outside that segment would escape whatever protects
/// it without anything noticing.
#[cfg(test)]
const OPERATOR_ROUTES: [&str; 8] = [
    "/api/v1/admin/users",
    "/api/v1/admin/activity",
    "/api/v1/admin/runtimes",
    "/api/v1/admin/sessions",
    // The same core operations the owner's own routes project. A second
    // address rather than second behaviour: an operator ending somebody else's
    // session should be doing it somewhere a gateway can see.
    "/api/v1/admin/sessions/{id}/stop",
    "/api/v1/admin/sessions/{id}/terminate",
    "/api/v1/admin/settings",
    "/api/v1/admin/settings/reset",
];

/// Routes this surface contributes.
pub fn router(state: Arc<AdminState>) -> Router {
    documented_router(state).into()
}

pub(super) fn documented_router(state: Arc<AdminState>) -> OpenApiRouter {
    let settings = settings::router(Arc::clone(&state.settings));
    OpenApiRouter::new()
        .routes(routes!(users))
        .routes(routes!(activity))
        .routes(routes!(runtimes))
        .routes(routes!(sessions))
        .routes(routes!(stop))
        .routes(routes!(terminate))
        .with_state(state)
        .merge(settings)
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    tag = "administration",
    params(("limit" = Option<u32>, Query)),
    responses((status = 200, description = "Registered users", body = [UserView]))
)]
async fn users(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<DirectoryParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Vec<UserView>>, AdminRequestFailure> {
    let Query(params) = params?;
    let users = state
        .users
        .list(
            &caller,
            DirectoryQuery {
                limit: params.limit.unwrap_or(DEFAULT_DIRECTORY_LIMIT),
            },
        )
        .await?;
    Ok(Json(users.into_iter().map(UserView::from).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/activity",
    tag = "administration",
    params(("actor" = Option<String>, Query), ("limit" = Option<u32>, Query)),
    responses((status = 200, description = "Activity records", body = [ActivityView]))
)]
async fn activity(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<ActivityParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Vec<ActivityView>>, AdminRequestFailure> {
    let Query(params) = params?;
    let entries = state
        .activity
        .list(
            &caller,
            ActivityQuery {
                actor: params.actor.map(SubjectId),
                limit: params.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT),
            },
        )
        .await?;
    Ok(Json(entries.into_iter().map(ActivityView::from).collect()))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/runtimes",
    tag = "administration",
    responses((status = 200, description = "Runtime inventory", body = InventoryView))
)]
async fn runtimes(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<InventoryView>, AdminProblem> {
    let inventory = state.sessions.runtime_inventory(&caller).await?;
    Ok(Json(InventoryView::render(inventory)))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/sessions",
    tag = "administration",
    params(
        ("status" = Option<String>, Query),
        ("limit" = Option<u32>, Query),
        ("cursor" = Option<String>, Query)
    ),
    responses((status = 200, description = "Cross-owner session page", body = [AdminSessionView]))
)]
async fn sessions(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<AdminSessionPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<(axum::http::HeaderMap, Json<Vec<AdminSessionView>>), AdminRequestFailure> {
    let Query(params) = params?;
    // Two core calls, each answering for its own capability. Reading everyone's
    // work and reading who those people are stay separate grants: the list is
    // opened by the first, and the second only decides whether the owners come
    // back named or as identifiers. Naming is scoped to the page, so a bounded
    // list stays bounded rather than dragging the whole directory behind it.
    let sessions = state
        .sessions
        .list_all_sessions(&caller, &params.store_query())
        .await?;
    let headers = params.next_headers("/api/v1/admin/sessions", sessions.next.as_ref());
    let owners: Vec<SubjectId> = sessions.items.iter().map(|s| s.owner.clone()).collect();
    let names = state.users.names_known_to(&caller, &owners).await?;
    Ok((
        headers,
        Json(
            sessions
                .items
                .into_iter()
                .map(|s| AdminSessionView::render(s, &names))
                .collect(),
        ),
    ))
}

/// End somebody's session.
///
/// Refusals keep the session surface's rendering: this route does address an
/// identifier, so a caller who may not act on it must not be able to learn
/// whether it exists by the shape of the answer.
#[utoipa::path(
    post,
    path = "/api/v1/admin/sessions/{id}/terminate",
    tag = "administration",
    params(("id" = String, Path)),
    responses((status = 204, description = "Session terminated by an operator"))
)]
async fn terminate(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Response, Problem> {
    let owner = state
        .sessions
        .terminate_any_session(&caller, &SessionId(id))
        .await?;
    Ok(acted(&caller, owner))
}

/// Release somebody's session's runtime, keeping the session.
#[utoipa::path(
    post,
    path = "/api/v1/admin/sessions/{id}/stop",
    tag = "administration",
    params(("id" = String, Path)),
    responses((status = 204, description = "Session stopped by an operator"))
)]
async fn stop(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Response, Problem> {
    let owner = state
        .sessions
        .stop_any_session(&caller, &SessionId(id))
        .await?;
    Ok(acted(&caller, owner))
}

/// The answer to a control, naming whose work it reached.
///
/// The subject rides in an extension rather than the body: the client asked
/// to stop a session and has nothing to learn from being told who owns it,
/// while whatever writes the record down cannot see it any other way. Omitted
/// when the two are the same, so the presence of the name means what it says.
fn acted(caller: &Caller, owner: SubjectId) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    if caller.is_acting_for_another(&owner) {
        response.extensions_mut().insert(ActedUpon(owner));
    }
    response
}

#[cfg(test)]
#[path = "admin_tests.rs"]
mod tests;
