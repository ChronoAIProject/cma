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
use std::time::SystemTime;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, AuthorizeEffectReconciliationCommand, Caller,
    CoreError, DirectoryQuery, LaunchEffectKind, OperationStore, PlatformSettingsService,
    ProviderView, RegisteredUser, RuntimeInventory, Session, SessionId, SessionService, SubjectId,
    TriggerService, TriggerServiceError, TriggerSupportLookup, TriggerSupportSnapshot,
    UserDirectory, DEFAULT_ACTIVITY_LIMIT, DEFAULT_DIRECTORY_LIMIT,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::TriggerSupportObserved;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::auth::{ActedUpon, AuthenticatedCaller};
use crate::rest::openapi_shapes;
use crate::rest::pagination::AdminSessionPageParams;
use crate::rest::trigger_openapi_shapes as trigger_schema;
use crate::rest::{
    idempotency, problem_document, problem_response, BodyProblem, Problem, QueryProblem,
};

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
    /// Redacted GitHub Trigger support projection, absent only when this
    /// deployment has no Trigger persistence configured.
    pub triggers: Option<Arc<TriggerService>>,
    /// Durable operation claims shared with every other idempotent REST command.
    pub operations: Arc<dyn OperationStore>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct AuthorizeEffectReconciliationRequest {
    expected_revision: u64,
    #[schema(value_type = trigger_schema::LaunchEffectKind)]
    effect: LaunchEffectKind,
    intent_key: String,
    expected_fence: u64,
    evidence_correlation_id: String,
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
#[serde(rename_all = "snake_case")]
pub enum RuntimeGenerationView {
    Legacy,
    Recorded,
    Malformed,
}

/// Whether provider metadata agrees with the durable runtime authority.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAuthorityView {
    Unaccounted,
    CoordinationDisabled,
    Legacy,
    Matches,
    Mismatch,
}

/// Why an exact runtime remains durably scheduled for cleanup.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCleanupView {
    UncommittedCandidate,
    RetiredAuthority,
}

/// One runtime in service, as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
pub struct RuntimeView {
    runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    generation_state: RuntimeGenerationView,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable_runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable_generation: Option<u64>,
    authority_state: RuntimeAuthorityView,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_reason: Option<RuntimeCleanupView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<String>,
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
                                .map(|r| {
                                    let (generation, generation_state) = match r.generation {
                                        layer2_core::ProviderRuntimeGeneration::Legacy => {
                                            (None, RuntimeGenerationView::Legacy)
                                        }
                                        layer2_core::ProviderRuntimeGeneration::Recorded(value) => {
                                            (Some(value), RuntimeGenerationView::Recorded)
                                        }
                                        layer2_core::ProviderRuntimeGeneration::Malformed => {
                                            (None, RuntimeGenerationView::Malformed)
                                        }
                                    };
                                    RuntimeView {
                                        unaccounted: r.is_unaccounted(),
                                        runtime: r.runtime,
                                        generation,
                                        generation_state,
                                        durable_runtime: r
                                            .durable_authority
                                            .as_ref()
                                            .and_then(|authority| authority.runtime_id.clone()),
                                        durable_generation: r
                                            .durable_authority
                                            .as_ref()
                                            .filter(|authority| authority.generation != 0)
                                            .map(|authority| authority.generation),
                                        authority_state: match r.authority_comparison {
                                            layer2_core::RuntimeAuthorityComparison::Unaccounted => {
                                                RuntimeAuthorityView::Unaccounted
                                            }
                                            layer2_core::RuntimeAuthorityComparison::CoordinationDisabled => {
                                                RuntimeAuthorityView::CoordinationDisabled
                                            }
                                            layer2_core::RuntimeAuthorityComparison::Legacy => {
                                                RuntimeAuthorityView::Legacy
                                            }
                                            layer2_core::RuntimeAuthorityComparison::Matches => {
                                                RuntimeAuthorityView::Matches
                                            }
                                            layer2_core::RuntimeAuthorityComparison::Mismatch => {
                                                RuntimeAuthorityView::Mismatch
                                            }
                                        },
                                        cleanup_reason: r.cleanup_reason.map(|reason| match reason {
                                            layer2_core::RuntimeCleanupReason::UncommittedCandidate => {
                                                RuntimeCleanupView::UncommittedCandidate
                                            }
                                            layer2_core::RuntimeCleanupReason::RetiredAuthority => {
                                                RuntimeCleanupView::RetiredAuthority
                                            }
                                        }),
                                        created_by: r.created_by,
                                        session: r.session.map(|s| s.0),
                                        owner: r.owner.map(|o| o.0),
                                        status: r.status,
                                    }
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

#[derive(Debug, Deserialize)]
pub struct TriggerSupportParams {
    #[serde(default)]
    launch_id: Option<String>,
    #[serde(default)]
    correlation_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TriggerSupportView {
    launch_id: String,
    owner: String,
    revision_digest: String,
    publisher: String,
    upstream_repository_id: u64,
    fork_repository_id: Option<u64>,
    github_account_id: Option<u64>,
    service_id: Option<String>,
    session_id: Option<String>,
    state: String,
    blocked_reason: Option<String>,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    terminal_at: Option<String>,
    lease: Option<TriggerSupportLeaseView>,
    cleanup: Option<TriggerSupportCleanupView>,
    effects: Vec<TriggerSupportEffectView>,
    timeline: Vec<TriggerSupportEventView>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerSupportLeaseView {
    fence: u64,
    acquired_at: String,
    expires_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerSupportCleanupView {
    retention_policy_id: String,
    delete_after: String,
    target_count: usize,
    planned_at: String,
    completed_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerSupportEffectView {
    kind: String,
    state: String,
    intent_key: String,
    attempt: u32,
    reconciliation_attempt: u32,
    fence: u64,
    provider_request_id: Option<String>,
    external_resource_ids: std::collections::BTreeMap<String, String>,
    failure_class: Option<String>,
    failure_code: Option<String>,
    failure_correlation_id: Option<String>,
    next_attempt_at: Option<String>,
    updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerSupportEventView {
    sequence: u64,
    occurred_at: String,
    correlation_id: String,
    #[schema(value_type = Object)]
    kind: serde_json::Value,
}

impl From<TriggerSupportSnapshot> for TriggerSupportView {
    fn from(value: TriggerSupportSnapshot) -> Self {
        Self {
            launch_id: value.launch_id.0,
            owner: value.owner.0,
            revision_digest: value.revision_digest,
            publisher: value.publisher.0,
            upstream_repository_id: value.upstream_repository_id,
            fork_repository_id: value.fork_repository_id,
            github_account_id: value.github_account_id,
            service_id: value.service_id,
            session_id: value.session_id,
            state: enum_code(value.state),
            blocked_reason: value.blocked_reason.map(enum_code),
            created_at: value.created_at,
            updated_at: value.updated_at,
            started_at: value.started_at,
            terminal_at: value.terminal_at,
            lease: value.lease.map(|lease| TriggerSupportLeaseView {
                fence: lease.fence,
                acquired_at: lease.acquired_at,
                expires_at: lease.expires_at,
            }),
            cleanup: value.cleanup.map(|cleanup| TriggerSupportCleanupView {
                retention_policy_id: cleanup.retention_policy_id,
                delete_after: cleanup.delete_after,
                target_count: cleanup.target_count,
                planned_at: cleanup.planned_at,
                completed_at: cleanup.completed_at,
            }),
            effects: value
                .effects
                .into_iter()
                .map(|effect| TriggerSupportEffectView {
                    kind: enum_code(effect.kind),
                    state: enum_code(effect.state),
                    intent_key: effect.intent_key,
                    attempt: effect.attempt,
                    reconciliation_attempt: effect.reconciliation_attempt,
                    fence: effect.fence,
                    provider_request_id: effect.provider_request_id,
                    external_resource_ids: effect.external_resource_ids,
                    failure_class: effect.failure_class.map(enum_code),
                    failure_code: effect.failure_code,
                    failure_correlation_id: effect.failure_correlation_id,
                    next_attempt_at: effect.next_attempt_at,
                    updated_at: effect.updated_at,
                })
                .collect(),
            timeline: value
                .timeline
                .into_iter()
                .map(|event| TriggerSupportEventView {
                    sequence: event.sequence,
                    occurred_at: event.occurred_at,
                    correlation_id: event.correlation_id,
                    kind: serde_json::to_value(event.kind)
                        .expect("the closed support event serializes"),
                })
                .collect(),
        }
    }
}

fn enum_code(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("the closed support enum serializes")
        .as_str()
        .expect("the support enum uses a string wire shape")
        .to_string()
}

/// Every path this surface serves.
///
/// Named once so the router and the rule about them cannot drift: the whole
/// surface lives under one segment precisely so a gateway can restrict it as a
/// unit, and a route added outside that segment would escape whatever protects
/// it without anything noticing.
#[cfg(test)]
const OPERATOR_ROUTES: [&str; 10] = [
    "/api/v1/admin/users",
    "/api/v1/admin/activity",
    "/api/v1/admin/triggers/support",
    "/api/v1/admin/triggers/launches/{launch_id}/reconcile",
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
        .routes(routes!(trigger_support))
        .routes(routes!(authorize_effect_reconciliation))
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
    path = "/api/v1/admin/triggers/support",
    tag = "administration",
    params(
        ("launch_id" = Option<String>, Query),
        ("correlation_id" = Option<String>, Query)
    ),
    responses(
        (status = 200, description = "Redacted Trigger Launch support timeline", body = TriggerSupportView),
        (status = 503, description = "Trigger support storage is not configured")
    )
)]
async fn trigger_support(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<TriggerSupportParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, Response> {
    let Query(params) = params.map_err(|error| QueryProblem(error).into_response())?;
    let lookup = match (params.launch_id, params.correlation_id) {
        (Some(id), None) => layer2_core::LaunchId::parse(id)
            .map(TriggerSupportLookup::Launch)
            .map_err(|_| invalid_support_lookup()),
        (None, Some(correlation)) => {
            TriggerSupportLookup::correlation(correlation).map_err(|_| invalid_support_lookup())
        }
        _ => Err(invalid_support_lookup()),
    }?;
    let requested_correlation = match &lookup {
        TriggerSupportLookup::CorrelationId(correlation_id) => Some(correlation_id.clone()),
        TriggerSupportLookup::Launch(_) => None,
    };
    let snapshot = state
        .triggers
        .as_ref()
        .ok_or_else(trigger_support_unavailable)?
        .support(&caller, lookup)
        .await
        .map_err(trigger_support_problem)?;
    let observed = TriggerSupportObserved {
        launch_id: snapshot.launch_id.clone(),
        correlation_id: requested_correlation,
    };
    let mut response = Json(TriggerSupportView::from(snapshot)).into_response();
    response.extensions_mut().insert(observed);
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/triggers/launches/{launch_id}/reconcile",
    tag = "administration",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = AuthorizeEffectReconciliationRequest,
    responses(
        (status = 202, description = "Exact fenced effect authorized for read-only reconciliation"),
        (status = 503, description = "Trigger recovery storage is not configured")
    )
)]
async fn authorize_effect_reconciliation(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<AuthorizeEffectReconciliationRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-effect-recovery-authorize:{launch_id}");
    let (guard, body, _) = match idempotency::required_command(
        key,
        Arc::clone(&state.operations),
        &caller,
        operation,
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, Response> = async {
        let body = body.map_err(|error| error.into_response())?;
        let timestamp = layer2_core::rfc3339_at(SystemTime::now());
        let launch = state
            .triggers
            .as_ref()
            .ok_or_else(trigger_support_unavailable)?
            .authorize_effect_reconciliation(
                &caller,
                &layer2_core::LaunchId::parse(launch_id)
                    .map_err(|_| trigger_support_problem(TriggerServiceError::NotFound))?,
                AuthorizeEffectReconciliationCommand {
                    expected_revision: body.expected_revision,
                    effect: body.effect,
                    intent_key: body.intent_key,
                    expected_fence: body.expected_fence,
                    evidence_correlation_id: body.evidence_correlation_id,
                    now: timestamp,
                    correlation_id: headers
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string)
                        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                },
            )
            .await
            .map_err(trigger_support_problem)?;
        Ok(effect_reconciliation_accepted(&caller, launch.owner))
    }
    .await;
    guard.finish(result).await
}

/// Return no contributor projection from a cross-owner recovery control.
/// Ownership is retained only in the response extension consumed by audit.
fn effect_reconciliation_accepted(caller: &Caller, owner: SubjectId) -> Response {
    let mut response = StatusCode::ACCEPTED.into_response();
    if caller.is_acting_for_another(&owner) {
        response.extensions_mut().insert(ActedUpon(owner));
    }
    response
}

fn trigger_support_unavailable() -> Response {
    let status = StatusCode::SERVICE_UNAVAILABLE;
    problem_response(
        status,
        problem_document(
            status,
            "trigger-support-unavailable",
            "GitHub Trigger support data is not configured in this deployment.",
            "trigger_support_unavailable",
        ),
    )
}

fn invalid_support_lookup() -> Response {
    let status = StatusCode::BAD_REQUEST;
    problem_response(
        status,
        problem_document(
            status,
            "invalid-trigger-support-lookup",
            "Provide exactly one valid launch_id or correlation_id.",
            "invalid_trigger_support_lookup",
        ),
    )
}

fn trigger_support_problem(error: TriggerServiceError) -> Response {
    match error {
        TriggerServiceError::Forbidden => NotAnOperator.into_response(),
        other => super::triggers::TriggerProblem(other).into_response(),
    }
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
