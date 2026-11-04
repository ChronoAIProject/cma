//! CMA Agent collection, detail, creation, and deletion routes.

use std::{future::Future, sync::Arc};

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use layer2_core::profiles::AgentProfileRef;
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{AgentListQuery, AgentOrigin, AgentStatus, CreateAgent};
use layer2_core::{AgentId, AgentProfileId, Caller, Capability, SandboxId, SubjectId, WorkspaceId};
use serde::Deserialize;

#[path = "agent_state.rs"]
mod agent_state;
use super::agent_lifecycle::{
    AgentLifecycleCreateReceipt, AgentLifecycleCreateRequest, AgentLifecycleProvisioningError,
};
use super::agent_openapi_shapes::{AgentListItemView, AgentView, CreateAgentRequest};
use super::agent_shared_shapes::{
    AccessibleAgentListItemView, AccessibleAgentPageView, AccessibleAgentView,
};
use super::idempotency::{self, IdempotencyKey, RequestBody};
use super::instance_problem::InstanceProblem;
use super::BodyProblem;
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
pub use agent_state::AgentRoutesState;

const DEFAULT_PAGE_LIMIT: u16 = 200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentPageParams {
    organization: Option<String>,
    sandbox: Option<String>,
    workspace: Option<String>,
    status: Option<AgentStatus>,
    cursor: Option<String>,
    limit: Option<u16>,
}

impl AgentPageParams {
    fn into_parts(
        self,
    ) -> Result<(Option<SubjectId>, AgentListQuery, PageRequest), InstanceProblem> {
        if self.organization.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 128 || value != value.trim()
        }) || self.sandbox.as_ref().is_some_and(|value| value.is_empty())
            || self
                .workspace
                .as_ref()
                .is_some_and(|value| value.is_empty())
            || self.cursor.as_ref().is_some_and(String::is_empty)
        {
            return Err(InstanceProblem::InvalidQuery);
        }
        let page = PageRequest::new(self.limit.unwrap_or(DEFAULT_PAGE_LIMIT), self.cursor)
            .map_err(|_| InstanceProblem::InvalidQuery)?;
        Ok((
            self.organization.map(SubjectId::new),
            AgentListQuery {
                sandbox_id: self.sandbox.map(SandboxId),
                workspace_id: self.workspace.map(WorkspaceId),
                status: self.status,
            },
            page,
        ))
    }
}

pub(super) fn routes() -> Router<AgentRoutesState> {
    Router::new()
        .route("/api/v1/workspaces/{id}/agents", post(create))
        .route("/api/v1/agents", get(list).post(create_lifecycle))
        .route(
            "/api/v1/desktop-capability",
            get(super::desktop_creation::read),
        )
        .route(
            "/api/v1/agent-lifecycle/{id}",
            get(read_lifecycle_operation),
        )
        .route("/api/v1/agents/{id}", get(read).delete(delete_agent))
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

/// Create an Agent from one inline lifecycle request.  The provisioning port
/// owns the sandbox/workspace/agent orchestration; this boundary never accepts
/// profile, worktree, pod, or runtime identifiers. An optional owned workspace
/// selects an existing context; the service verifies its creation authority.
#[utoipa::path(
    post,
    path = "/api/v1/agents",
    tag = "agents",
    params(("Idempotency-Key" = String, Header)),
    request_body = AgentLifecycleCreateRequest,
    responses(
        (status = 202, body = AgentLifecycleCreateReceipt),
        (status = 422, body = crate::rest::ProblemDocument),
        (status = 503, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn create_lifecycle(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    context: Option<Extension<layer2_core::resource_access::CallerContext>>,
    credential: Option<Extension<DelegatedCredential>>,
    key: IdempotencyKey,
    body: Result<RequestBody<AgentLifecycleCreateRequest>, BodyProblem>,
) -> Response {
    if let Err(problem) = require_use(&caller) {
        return problem.into_response();
    }
    let Some(provisioner) = state.lifecycle.clone() else {
        return AgentLifecycleProvisioningError::Unavailable.into_response();
    };
    // The lifecycle's permanent owner/key ledger owns admission and resumption.
    // An outer cached 202 would suppress requeue after connecting services.
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let request = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if request.desktop && request.initial_message.is_none() {
        return AgentLifecycleProvisioningError::Invalid(
            "desktop creation requires initial_message".into(),
        )
        .into_response();
    }
    provisioner
        .create(
            &caller,
            context.as_ref().map(|Extension(value)| value),
            credential.as_ref().map(|Extension(value)| value.value()),
            request,
            &key,
        )
        .await
        .map(|receipt| (StatusCode::ACCEPTED, Json(receipt)).into_response())
        .unwrap_or_else(IntoResponse::into_response)
}

/// Compose all owner-facing Agent routes against one shared service set.
pub fn agent_router(state: AgentRoutesState) -> Router {
    routes()
        .merge(super::agent_operate::routes())
        .merge(super::agent_reads::routes())
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
    params(
        ("organization" = Option<String>, Query),
        ("sandbox" = Option<String>, Query),
        ("workspace" = Option<String>, Query),
        ("status" = Option<String>, Query),
        ("cursor" = Option<String>, Query),
        ("limit" = Option<u16>, Query)
    ),
    responses((status = 200, body = AccessibleAgentPageView))
)]
pub(crate) async fn list(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    params: Result<Query<AgentPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AccessibleAgentPageView>, InstanceProblem> {
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
        }));
    }
    let stored = state.reads.list(&caller, &query, &page).await?;
    Ok(Json(AccessibleAgentPageView {
        items: stored
            .items
            .into_iter()
            .map(|agent| {
                AccessibleAgentListItemView::Personal(Box::new(AgentListItemView::from(agent)))
            })
            .collect(),
        next: stored.next,
    }))
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
) -> Result<Json<AccessibleAgentView>, InstanceProblem> {
    if let Some(resource) = resource {
        return state
            .reads
            .detail_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &AgentId(id),
            )
            .await
            .map(AccessibleAgentView::from)
            .map(Json)
            .map_err(InstanceProblem::from);
    }
    read_view(&state, &caller, &AgentId(id))
        .await
        .map(|view| Json(AccessibleAgentView::Personal(Box::new(view))))
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
    execute_empty_command(
        &state,
        &caller,
        key,
        format!("agent:delete:{id}"),
        move || async move {
            command_state
                .agents
                .delete(&operation_caller, &AgentId(id))
                .await
                .map_err(InstanceProblem::from_delete)?;
            Ok(StatusCode::ACCEPTED.into_response())
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

pub(super) async fn execute_command<T, F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    if let Err(problem) = require_use(caller) {
        return problem.into_response();
    }
    let (guard, body, _) = match idempotency::required_command(
        key,
        Arc::clone(&state.operation_records),
        caller,
        operation,
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, Response> = match body {
        Ok(body) => execute(body).await.map_err(IntoResponse::into_response),
        Err(error) => Err(error.into_response()),
    };
    guard.finish(result).await
}

pub(super) async fn execute_empty_command<F, Fut>(
    state: &AgentRoutesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response, InstanceProblem>>,
{
    if let Err(problem) = require_use(caller) {
        return problem.into_response();
    }
    let guard = match idempotency::required_empty_command(
        key,
        Arc::clone(&state.operation_records),
        caller,
        operation,
    )
    .await
    {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    guard
        .finish(execute().await.map_err(IntoResponse::into_response))
        .await
}

fn require_use(caller: &Caller) -> Result<(), InstanceProblem> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(InstanceProblem::Forbidden)
}
