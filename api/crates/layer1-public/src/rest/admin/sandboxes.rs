//! Cross-owner Sandbox inventory, detail, events, and retirement.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::reviewed_retirement::{
    ReviewedRetirementMember, ReviewedRetirementOutcome, ReviewedRetirementReceipt,
};
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{
    AgentAdminSummary, AgentStatus, Sandbox, SandboxAdminQuery, SandboxAdminSummary,
    SandboxAdminView, SandboxEvent, SandboxStatus, WorkspaceAdminSummary, WorkspaceStatus,
};
use layer2_core::{Capability, SandboxId, SubjectId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AuthenticatedCaller;

use super::super::instances_openapi_shapes::SandboxView;
use super::super::sandbox_runtime_update::SandboxRuntimeUpdateView;
use super::{AdminState, CollectionProblem, ProtectedKind, ProtectedProblem};

const DEFAULT_EVENT_LIMIT: u16 = 200;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminAlwaysOnRequest {
    always_on: bool,
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetireParameters {
    expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SandboxPageParams {
    owner: Option<String>,
    status: Option<SandboxStatus>,
    runtime_version: Option<String>,
    always_on: Option<bool>,
    cursor: Option<String>,
    limit: Option<u16>,
}

impl SandboxPageParams {
    fn into_query(self) -> Result<SandboxAdminQuery, CollectionProblem> {
        if self.cursor.as_ref().is_some_and(String::is_empty) {
            return Err(CollectionProblem::InvalidRequest);
        }
        let page = PageRequest::new(self.limit.unwrap_or(200), self.cursor)
            .map_err(|_| CollectionProblem::InvalidRequest)?;
        Ok(SandboxAdminQuery {
            owner: self.owner.map(SubjectId),
            status: self.status,
            runtime_version: self.runtime_version,
            always_on: self.always_on,
            page,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxSummaryView {
    id: String,
    owner: String,
    owner_name: Option<String>,
    name: String,
    #[schema(value_type = String)]
    status: SandboxStatus,
    runtime_version: Option<String>,
    agent_count: u64,
    last_activity_at: String,
    retired_at: Option<String>,
}

impl From<SandboxAdminSummary> for SandboxSummaryView {
    fn from(value: SandboxAdminSummary) -> Self {
        Self {
            id: value.id.0,
            owner: value.owner.0,
            owner_name: None,
            name: value.name,
            status: value.status,
            runtime_version: value.runtime_version,
            agent_count: value.agent_count,
            last_activity_at: value.last_activity_at,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxPageView {
    items: Vec<SandboxSummaryView>,
    next: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkspaceSummaryView {
    id: String,
    name: String,
    #[schema(value_type = String)]
    status: WorkspaceStatus,
    profile_id: String,
    profile_revision: u32,
}

impl From<WorkspaceAdminSummary> for WorkspaceSummaryView {
    fn from(value: WorkspaceAdminSummary) -> Self {
        Self {
            id: value.id.0,
            name: value.name,
            status: value.status,
            profile_id: value.profile_id,
            profile_revision: value.profile_revision,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentSummaryView {
    id: String,
    workspace_id: String,
    worktree_id: String,
    #[schema(value_type = String)]
    status: AgentStatus,
    profile_id: String,
    profile_revision: u32,
    title: Option<String>,
    updated_at: String,
}

impl From<AgentAdminSummary> for AgentSummaryView {
    fn from(value: AgentAdminSummary) -> Self {
        Self {
            id: value.id.0,
            workspace_id: value.workspace_id.0,
            worktree_id: value.worktree_id.0,
            status: value.status,
            profile_id: value.profile_id,
            profile_revision: value.profile_revision,
            title: value.title,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxDetailView {
    owner_name: Option<String>,
    lifecycle: super::super::instances_openapi_shapes::SandboxLifecycleView,
    runtime_update: Option<SandboxRuntimeUpdateView>,
    #[schema(value_type = Object)]
    sandbox: Sandbox,
    workspaces: Vec<WorkspaceSummaryView>,
    agents: Vec<AgentSummaryView>,
}

impl From<SandboxAdminView> for SandboxDetailView {
    fn from(value: SandboxAdminView) -> Self {
        Self {
            owner_name: None,
            runtime_update: None,
            lifecycle: (&value.sandbox).into(),
            sandbox: value.sandbox,
            workspaces: value.workspaces.into_iter().map(Into::into).collect(),
            agents: value.agents.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventParams {
    after: Option<u64>,
    limit: Option<u16>,
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/sandboxes", get(list))
        .route("/api/v1/admin/sandboxes/{id}", get(read))
        .route("/api/v1/admin/sandboxes/{id}/always-on", post(always_on))
        .route("/api/v1/admin/sandboxes/{id}/events", get(events))
        .route("/api/v1/admin/sandboxes/{id}/retire", post(retire))
        .route(
            "/api/v1/admin/sandboxes/{id}/retire-reviewed",
            post(retire_reviewed),
        )
        .route(
            "/api/v1/admin/sandboxes/{id}/retire-reviewed/{batch}/{member}",
            get(reviewed_retirement_receipt),
        )
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/sandboxes/{id}/always-on",
    tag = "administration",
    params(("id" = String, Path)),
    request_body = AdminAlwaysOnRequest,
    responses((status = 200, body = SandboxView), (status = 403, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument))
)]
/// Approve or revoke Always on with a revision check and a configure activity
/// containing the actor, Sandbox id, new value and resulting revision.
pub(crate) async fn always_on(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<AdminAlwaysOnRequest>,
) -> Result<Json<SandboxView>, CollectionProblem> {
    let sandbox = state
        .sandboxes
        .set_always_on(
            &caller,
            &SandboxId(id),
            body.expected_revision,
            body.always_on,
        )
        .await?;
    Ok(Json(SandboxView::from(&sandbox)))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/sandboxes",
    tag = "administration",
    params(
        ("owner" = Option<String>, Query),
        ("status" = Option<String>, Query),
        ("runtime_version" = Option<String>, Query),
        ("always_on" = Option<bool>, Query, description = "True selects explicit Always on exceptions; false includes unset preferences."),
        ("cursor" = Option<String>, Query),
        ("limit" = Option<u16>, Query)
    ),
    responses((status = 200, body = SandboxPageView), (status = 403, body = crate::rest::ProblemDocument))
)]
pub(crate) async fn list(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<SandboxPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<SandboxPageView>, CollectionProblem> {
    let query = params
        .map_err(|_| CollectionProblem::InvalidRequest)?
        .0
        .into_query()?;
    let page = state.sandboxes.list_sandboxes(&caller, &query).await?;
    let subjects: Vec<_> = page.items.iter().map(|row| row.owner.clone()).collect();
    let names = super::super::public_names::resolve(&state.users, &subjects).await;
    Ok(Json(SandboxPageView {
        items: page
            .items
            .into_iter()
            .map(|row| {
                let mut view = SandboxSummaryView::from(row);
                view.owner_name = names.get(&view.owner).cloned();
                view
            })
            .collect(),
        next: page.next,
    }))
}

#[utoipa::path(get, path = "/api/v1/admin/sandboxes/{id}", tag = "administration", params(("id" = String, Path)), responses((status = 200, body = SandboxDetailView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<SandboxDetailView>, ProtectedProblem> {
    let value = state
        .sandboxes
        .read_sandbox(&caller, &SandboxId(id))
        .await
        .map_err(ProtectedProblem::sandbox)?;
    let names = super::super::public_names::resolve(
        &state.users,
        std::slice::from_ref(&value.sandbox.owner),
    )
    .await;
    let mut view = SandboxDetailView::from(value);
    view.runtime_update = state
        .runtime_updates
        .as_ref()
        .map(|service| SandboxRuntimeUpdateView::new(&view.sandbox, service));
    view.owner_name = names.get(&view.sandbox.owner.0).cloned();
    Ok(Json(view))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/sandboxes/{id}/events",
    tag = "administration",
    params(("id" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = [Object]), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(crate) async fn events(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    params: Result<Query<EventParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Vec<SandboxEvent>>, ProtectedProblem> {
    if !caller.authorization.allows(Capability::ReadAllSandboxes) {
        return Err(ProtectedProblem::Hidden(ProtectedKind::Sandbox));
    }
    let params = params.map_err(|_| ProtectedProblem::InvalidRequest)?.0;
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if !(1..=DEFAULT_EVENT_LIMIT).contains(&limit) {
        return Err(ProtectedProblem::InvalidRequest);
    }
    state
        .sandboxes
        .sandbox_events(&caller, &SandboxId(id), params.after.unwrap_or(0), limit)
        .await
        .map(Json)
        .map_err(ProtectedProblem::sandbox)
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/sandboxes/{id}/retire",
    tag = "administration",
    params(("id" = String, Path), ("expected_revision" = Option<u64>, Query)),
    responses(
        (status = 200, body = Object),
        (status = 400, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn retire(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    params: Result<Query<RetireParameters>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Sandbox>, ProtectedProblem> {
    if !caller.authorization.allows(Capability::ControlAnySandbox) {
        return Err(ProtectedProblem::Hidden(ProtectedKind::Sandbox));
    }
    let Query(params) = params.map_err(|_| ProtectedProblem::InvalidRequest)?;
    state
        .sandboxes
        .retire_sandbox(&caller, &SandboxId(id), params.expected_revision)
        .await
        .map(|result| Json(result.sandbox))
        .map_err(ProtectedProblem::sandbox)
}

async fn retire_reviewed(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<ReviewedRetirementMember>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ReviewedRetirementOutcome>, ProtectedProblem> {
    if !caller.authorization.allows(Capability::ControlAnySandbox) {
        return Err(ProtectedProblem::Hidden(ProtectedKind::Sandbox));
    }
    let Json(member) = body.map_err(|_| ProtectedProblem::InvalidRequest)?;
    state
        .sandboxes
        .admit_reviewed_retirement(&caller, &SandboxId(id), &member)
        .await
        .map(Json)
        .map_err(ProtectedProblem::sandbox)
}

async fn reviewed_retirement_receipt(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, batch, member)): Path<(String, String, String)>,
) -> Result<Json<ReviewedRetirementReceipt>, ProtectedProblem> {
    state
        .sandboxes
        .reviewed_retirement_receipt(&caller, &SandboxId(id), &batch, &member)
        .await
        .map(Json)
        .map_err(ProtectedProblem::sandbox)
}

#[cfg(test)]
#[path = "always_on_tests.rs"]
mod tests;
