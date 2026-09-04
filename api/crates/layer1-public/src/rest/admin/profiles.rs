//! Cross-owner Profile inventory and retirement.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::{
    ProfileAdminQuery, ProfileAdminSummary, ProfileKind, ProfileLocator, ProfilePageRequest,
    ProfileVisibility,
};
use layer2_core::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AuthenticatedCaller;

use super::{AdminState, CollectionProblem, ProtectedKind, ProtectedProblem};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfilePageParams {
    kind: Option<ProfileKind>,
    visibility: Option<ProfileVisibility>,
    owner: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
}

impl ProfilePageParams {
    fn into_query(self) -> Result<ProfileAdminQuery, CollectionProblem> {
        let limit = self.limit.unwrap_or(100);
        if limit == 0 || limit > 200 || self.cursor.as_ref().is_some_and(String::is_empty) {
            return Err(CollectionProblem::InvalidRequest);
        }
        Ok(ProfileAdminQuery {
            kind: self.kind,
            visibility: self.visibility,
            owner: self.owner.map(SubjectId),
            page: ProfilePageRequest {
                limit,
                cursor: self.cursor,
            },
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProfileSummaryView {
    #[schema(value_type = String)]
    kind: ProfileKind,
    id: String,
    owner: String,
    name: String,
    #[schema(value_type = String)]
    visibility: ProfileVisibility,
    updated_at: String,
    retired_at: Option<String>,
}

impl From<ProfileAdminSummary> for ProfileSummaryView {
    fn from(value: ProfileAdminSummary) -> Self {
        Self {
            kind: value.kind,
            id: value.id,
            owner: value.owner.0,
            name: value.name,
            visibility: value.visibility,
            updated_at: value.updated_at,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProfilePageView {
    items: Vec<ProfileSummaryView>,
    next: Option<String>,
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/profiles", get(list))
        .route("/api/v1/admin/profiles/{kind}/{id}/retire", post(retire))
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/profiles",
    tag = "administration",
    params(
        ("kind" = Option<String>, Query),
        ("visibility" = Option<String>, Query),
        ("owner" = Option<String>, Query),
        ("cursor" = Option<String>, Query),
        ("limit" = Option<u16>, Query)
    ),
    responses((status = 200, body = ProfilePageView), (status = 403, body = crate::rest::ProblemDocument))
)]
pub(crate) async fn list(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<ProfilePageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ProfilePageView>, CollectionProblem> {
    let query = params
        .map_err(|_| CollectionProblem::InvalidRequest)?
        .0
        .into_query()?;
    let page = state.sandboxes.list_profiles(&caller, &query).await?;
    Ok(Json(ProfilePageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next,
    }))
}

fn locator(kind: &str, id: String) -> Result<ProfileLocator, ProtectedProblem> {
    match kind {
        "agent" => Ok(ProfileLocator::Agent(AgentProfileId(id))),
        "workspace" => Ok(ProfileLocator::Workspace(WorkspaceProfileId(id))),
        "sandbox" => Ok(ProfileLocator::Sandbox(SandboxProfileId(id))),
        _ => Err(ProtectedProblem::Hidden(ProtectedKind::Profile)),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/profiles/{kind}/{id}/retire",
    tag = "administration",
    params(("kind" = String, Path), ("id" = String, Path)),
    responses(
        (status = 200, body = ProfileSummaryView),
        (status = 404, body = crate::rest::ProblemDocument),
        (status = 409, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn retire(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<ProfileSummaryView>, ProtectedProblem> {
    state
        .sandboxes
        .retire_profile(&caller, &locator(&kind, id)?)
        .await
        .map(Into::into)
        .map(Json)
        .map_err(ProtectedProblem::profile)
}
