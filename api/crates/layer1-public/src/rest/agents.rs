//! CMA Agent collection, detail, creation, and deletion routes.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use layer2_core::profiles::AgentProfileRef;
use layer2_core::sandbox::{AgentDetailReading, AgentOrigin, CreateAgent};
use layer2_core::{AgentId, AgentProfileId, Caller, Capability, WorkspaceId};

#[path = "agent_command_execution.rs"]
mod command_execution;
use command_execution::execute_empty_command_with_claim;
pub(super) use command_execution::{
    execute_command, execute_command_with_claim, execute_empty_command,
};
#[path = "agent_state.rs"]
mod agent_state;
mod inventory;
pub(super) mod lifecycle_creation;
use super::agent_lifecycle::AgentLifecycleProvisioningError;
use super::agent_openapi_shapes::{AgentView, CreateAgentRequest};
use super::agent_shared_shapes::{
    AccessibleAgentListItemView, AccessibleAgentPageView, AccessibleAgentView,
};
use super::idempotency::{IdempotencyKey, RequestBody};
use super::instance_problem::InstanceProblem;
use super::profiles_problem::ProfileProblem;
use super::profiles_sandbox::readable_revision;
use super::BodyProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
pub use agent_state::AgentRoutesState;
use inventory::personal_agent_page;
pub(crate) use inventory::AgentPageParams;
pub use inventory::{personal_agent_inventory_router, PersonalAgentInventory};

pub(super) fn routes() -> Router<AgentRoutesState> {
    let list = get(list).layer(axum::middleware::from_fn(agent_state::list_private_headers));
    Router::new()
        .route("/api/v1/workspaces/{id}/agents", post(create))
        .route(
            "/api/v1/agents",
            list.post(lifecycle_creation::create_lifecycle),
        )
        .route(
            "/api/v1/desktop-capability",
            get(super::desktop_creation::read),
        )
        .route(
            "/api/v1/agent-lifecycle/{id}",
            get(read_lifecycle_operation),
        )
        .route(
            "/api/v1/agent-lifecycle/{id}/cancel",
            post(cancel_lifecycle_operation),
        )
        .route("/api/v1/agents/{id}", get(read).delete(delete_agent))
        .route(
            "/api/v1/agents/{id}/restart",
            post(super::owner_agent_restart::restart),
        )
        .route(
            "/api/v1/agents/{id}/suggestions",
            get(super::suggestions::read),
        )
        .route(
            "/api/v1/agents/{id}/recover-worktree",
            post(super::agent_worktree_recovery::recover_worktree),
        )
        .route(
            "/api/v1/agents/{id}/message-feedback",
            get(super::message_feedback::list),
        )
        .route(
            "/api/v1/agents/{id}/messages/{message_id}/feedback",
            put(super::message_feedback::set).get(super::message_feedback_reasons::get),
        )
        .route(
            "/api/v1/agents/{id}/messages/{message_id}/feedback/reasons",
            put(super::message_feedback_reasons::set),
        )
        .route(
            "/api/v1/agents/{id}/skills",
            get(super::agent_skills::read).post(super::agent_skills::install),
        )
        .route(
            "/api/v1/agents/{id}/preferences",
            get(super::agent_preferences::read).patch(super::agent_preferences::update),
        )
        .route(
            "/api/v1/agents/{id}/settings",
            get(super::agent_settings::read).patch(super::agent_settings::update),
        )
}

/// Read a durable Agent lifecycle operation and its phase events.
#[utoipa::path(
    get,
    path = "/api/v1/agent-lifecycle/{id}",
    tag = "agents",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = crate::rest::AgentLifecycleOperationView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn read_lifecycle_operation(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(operation_id): Path<String>,
) -> Response {
    let Some(reader) = state.lifecycle_operations else {
        return AgentLifecycleProvisioningError::Unavailable.into_response();
    };
    reader
        .read(&caller, &operation_id)
        .await
        .map(|view| (StatusCode::OK, Json(view)).into_response())
        .unwrap_or_else(IntoResponse::into_response)
}

/// Cancel an owner-scoped creation only when its durable evidence still excludes allocation.
#[utoipa::path(post, path = "/api/v1/agent-lifecycle/{id}/cancel", tag = "agents",
    params(("id" = String, Path)),
    responses((status = 200, body = crate::rest::AgentLifecycleOperationView),
        (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn cancel_lifecycle_operation(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(operation_id): Path<String>,
) -> Response {
    let Some(reader) = state.lifecycle_operations else {
        return AgentLifecycleProvisioningError::Unavailable.into_response();
    };
    reader
        .cancel_unallocated(&caller, &operation_id)
        .await
        .map(|view| (StatusCode::OK, Json(view)).into_response())
        .unwrap_or_else(IntoResponse::into_response)
}

/// Compose all owner-facing Agent routes against one shared service set.
pub fn agent_router(state: AgentRoutesState) -> Router {
    routes()
        .merge(super::agent_operate::routes())
        .merge(super::agent_reads::routes())
        .merge(super::agent_original_request::routes())
        .merge(super::agent_reply_preview::routes())
        .merge(super::agent_retained_output::routes())
        .with_state(state)
}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{id}/agents",
    tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = CreateAgentRequest,
    responses(
        (status = 201, body = AgentView),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn create(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(workspace_id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateAgentRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let operation_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent:create:{workspace_id}"),
        body,
        move |body| async move {
            let request = CreateAgent {
                profile: AgentProfileRef::new(
                    AgentProfileId(body.agent_profile.id),
                    body.agent_profile.revision,
                ),
                title: body.title,
                first_message: body.first_message,
                origin: AgentOrigin::Manual,
            };
            let workspace_id = WorkspaceId(workspace_id);
            let agent = if let Some(resource) = resource {
                command_state
                    .agents
                    .create_authorized(&resource.context, &workspace_id, request)
                    .await
            } else {
                command_state
                    .agents
                    .create(&operation_caller, &workspace_id, request)
                    .await
            }
            .map_err(InstanceProblem::from_create)?;
            let view = read_view(&command_state, &operation_caller, &agent.id).await?;
            Ok((StatusCode::CREATED, Json(view)).into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/agents",
    tag = "agents",
    params(AgentPageParams),
    responses((status = 200, body = AccessibleAgentPageView),
        (status = 400, description = "invalid_instance_query: invalid filters, cursor or limit; limit must be from 1 through 200", body = super::ProblemDocument, content_type = "application/problem+json"))
)]
pub(crate) async fn list(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    params: Result<Query<AgentPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, InstanceProblem> {
    let (organization, query, page) = params
        .map_err(|_| InstanceProblem::InvalidQuery)?
        .0
        .into_parts()?;
    if let Some(organization) = organization {
        let resource = resource.ok_or(InstanceProblem::Forbidden)?;
        let stored = state
            .reads
            .organization_list_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &organization,
                &query,
                &page,
            )
            .await?;
        return Ok(Json(AccessibleAgentPageView {
            items: stored
                .items
                .into_iter()
                .map(AccessibleAgentListItemView::from)
                .collect(),
            next: stored.next,
        })
        .into_response());
    }
    let stored = state.reads.list(&caller, &query, &page).await?;
    Ok(personal_agent_page(stored))
}

#[utoipa::path(
    get,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = AccessibleAgentView),
        (status = 404, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Response, InstanceProblem> {
    let id = AgentId(id);
    let detail = if let Some(resource) = resource {
        state
            .reads
            .detail_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await?
    } else {
        AgentDetailReading::Personal(Box::new(state.reads.lineage(&caller, &id).await?))
    };
    let (view, private) = match detail {
        AgentDetailReading::Personal(lineage) => {
            let readable = if let Some(profiles) = state.sandbox_profile_readability.as_deref() {
                let pinned = &lineage.sandbox.profile;
                match readable_revision(profiles, &caller, pinned.id.as_str(), pinned.revision)
                    .await
                {
                    Ok(revision) => Some(revision.digest == pinned.digest),
                    Err(ProfileProblem::Forbidden | ProfileProblem::NotFound) => Some(false),
                    Err(_) => None,
                }
            } else {
                None
            };
            let restart_available = state.owner_agent_restart.is_some()
                && lineage.agent.owner == caller.subject
                && caller.authorization.allows(Capability::UseOwnSandboxes)
                && lineage.sandbox.status == layer2_core::sandbox::SandboxStatus::Active
                && lineage
                    .agent
                    .plan_owner_restart(&lineage.worktree, layer2_core::now_rfc3339())
                    .is_ok();
            let mut view =
                AgentView::from_lineage(*lineage, None).with_sandbox_profile_readability(readable);
            view.restart_available = restart_available.then_some(true);
            (
                AccessibleAgentView::Personal(Box::new(view)),
                restart_available || state.sandbox_profile_readability.is_some(),
            )
        }
        detail => (AccessibleAgentView::from(detail), false),
    };
    let body = Json(view.with_agent_page(&state.agent_page_origin));
    let mut response = body.into_response();
    if private {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    Ok(response)
}

#[utoipa::path(
    delete,
    path = "/api/v1/agents/{id}",
    tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses(
        (status = 202, description = "Agent deletion scheduled"),
        (status = 409, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn delete_agent(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let operation_caller = caller.clone();
    let agent_id = AgentId(id.clone());
    execute_empty_command_with_claim(
        &state,
        &caller,
        key,
        format!("agent:delete:{id}"),
        move |_guard| {
            #[cfg(feature = "qualification-hooks")]
            let qualification = command_state.qualification_for(
                _guard,
                layer2_core::qualification::ControlOperation::Delete,
                &agent_id,
            );
            async move {
                #[cfg(feature = "qualification-hooks")]
                if let Some((checkpoint, context)) = qualification {
                    let scheduled = command_state
                        .agents
                        .delete_with_result(&operation_caller, &agent_id)
                        .await
                        .map_err(InstanceProblem::from_delete)?;
                    checkpoint
                        .after_schedule(context, (&scheduled).into())
                        .await;
                    return Ok(StatusCode::ACCEPTED.into_response());
                }
                command_state
                    .agents
                    .delete(&operation_caller, &agent_id)
                    .await
                    .map_err(InstanceProblem::from_delete)?;
                Ok(StatusCode::ACCEPTED.into_response())
            }
        },
    )
    .await
}

pub(super) async fn read_view(
    state: &AgentRoutesState,
    caller: &Caller,
    id: &AgentId,
) -> Result<AgentView, InstanceProblem> {
    let lineage = state.reads.lineage(caller, id).await?;
    Ok(AgentView::from_lineage(lineage, None))
}

fn require_use(caller: &Caller) -> Result<(), InstanceProblem> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(InstanceProblem::Forbidden)
}
