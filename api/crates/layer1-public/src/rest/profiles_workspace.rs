//! Workspace Profile REST resource.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::{
    ProfilePageRequest, ProfileStoreError, ProfileVisibility, WorkspaceProfile,
};
use layer2_core::{Capability, SubjectId};

use super::idempotency::{IdempotencyKey, RequestBody};
use super::profiles::{
    execute_command, execute_empty_command, generated_id_is_valid, profile_page, ProfilesState,
};
use super::profiles_openapi_shapes::*;
use super::profiles_problem::ProfileProblem;
use super::profiles_references::validate_agent_references;
use super::BodyProblem;
use crate::AuthenticatedCaller;

pub(super) fn routes() -> Router<ProfilesState> {
    Router::new()
        .route("/api/v1/workspace-profiles", post(create).get(list))
        .route("/api/v1/workspace-profiles/{id}", get(read).put(update))
        .route(
            "/api/v1/workspace-profiles/{id}/revisions",
            post(publish).get(list_revisions),
        )
        .route(
            "/api/v1/workspace-profiles/{id}/revisions/{revision}",
            get(read_revision),
        )
        .route(
            "/api/v1/workspace-profiles/{id}/visibility",
            axum::routing::put(set_visibility),
        )
        .route("/api/v1/workspace-profiles/{id}/retire", post(retire))
}

fn authorized(caller: &layer2_core::Caller) -> Result<(), ProfileProblem> {
    caller
        .require(Capability::ManageOwnProfiles)
        .map_err(|_| ProfileProblem::Forbidden)
}

async fn owned(
    state: &ProfilesState,
    owner: &SubjectId,
    id: &str,
) -> Result<WorkspaceProfile, ProfileProblem> {
    if !generated_id_is_valid(id, "wkp_") {
        return Err(ProfileProblem::NotFound);
    }
    state
        .workspaces
        .workspace_profile(&layer2_core::WorkspaceProfileId(id.to_owned()))
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .filter(|profile| &profile.owner == owner && !profile.internal)
        .ok_or(ProfileProblem::NotFound)
}

async fn latest(
    state: &ProfilesState,
    profile: &WorkspaceProfile,
) -> Result<Option<ProfileRevisionSummary>, ProfileProblem> {
    let page = state
        .workspaces
        .list_workspace_profile_revisions(
            &profile.id,
            &ProfilePageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?;
    Ok(page
        .items
        .into_iter()
        .next()
        .map(WorkspaceProfileRevisionView::from)
        .as_ref()
        .map(ProfileRevisionSummary::from))
}

async fn view(
    state: &ProfilesState,
    profile: WorkspaceProfile,
) -> Result<WorkspaceProfileView, ProfileProblem> {
    let revision = latest(state, &profile).await?;
    Ok(WorkspaceProfileView::new(profile, revision))
}

fn invalid(error: layer2_core::profiles::ProfileError) -> ProfileProblem {
    ProfileProblem::from_store(ProfileStoreError::Validation(error), "content")
}

#[utoipa::path(
    post,
    path = "/api/v1/workspace-profiles",
    tag = "workspace profiles",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateWorkspaceProfileRequest,
    responses((status = 201, body = WorkspaceProfileView), (status = 422, body = crate::rest::ProblemDocument))
)]
pub(super) async fn create(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateWorkspaceProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        "workspace-profile:create".into(),
        body,
        move |body| async move {
            let now = layer2_core::now_rfc3339();
            let profile = WorkspaceProfile {
                internal: false,
                id: layer2_core::WorkspaceProfileId::generate(),
                owner: owner.clone(),
                name: body.name,
                description: body.description,
                visibility: ProfileVisibility::Private,
                forked_from: None,
                uses_count: 0,
                draft: body.content.into(),
                draft_version: 1,
                created_at: now.clone(),
                updated_at: now,
                retired_at: None,
            };
            profile.validate().map_err(invalid)?;
            validate_agent_references(&command_state, &owner, None, &profile.draft.agent_profiles)
                .await?;
            command_state
                .workspaces
                .create_workspace_profile(profile.clone())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content.agent_profiles"))?;
            Ok((
                axum::http::StatusCode::CREATED,
                Json(WorkspaceProfileView::new(profile, None)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace-profiles",
    tag = "workspace profiles",
    params(("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = WorkspaceProfilePageView))
)]
pub(super) async fn list(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<ProfilePageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<WorkspaceProfilePageView>, ProfileProblem> {
    authorized(&caller)?;
    let page = profile_page(query.map_err(|_| ProfileProblem::InvalidPage)?.0)?;
    let stored = state
        .workspaces
        .list_workspace_profiles(Some(&caller.subject), false, &page)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?;
    let mut items = Vec::with_capacity(stored.items.len());
    for profile in stored.items {
        items.push(view(&state, profile).await?);
    }
    Ok(Json(WorkspaceProfilePageView {
        items,
        next: stored.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace-profiles/{id}",
    tag = "workspace profiles",
    params(("id" = String, Path)),
    responses((status = 200, body = WorkspaceProfileView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn read(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceProfileView>, ProfileProblem> {
    authorized(&caller)?;
    Ok(Json(
        view(&state, owned(&state, &caller.subject, &id).await?).await?,
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/workspace-profiles/{id}",
    tag = "workspace profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateWorkspaceProfileRequest,
    responses((status = 200, body = WorkspaceProfileView), (status = 409, body = crate::rest::ProblemDocument))
)]
pub(super) async fn update(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateWorkspaceProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("workspace-profile:update:{id}"),
        body,
        move |body| async move {
            let mut profile = owned(&command_state, &owner, &id).await?;
            profile.draft_version = body
                .draft_version
                .checked_add(1)
                .ok_or(ProfileProblem::VersionConflict)?;
            profile.name = body.name.unwrap_or(profile.name);
            profile.description = body.description.unwrap_or(profile.description);
            profile.draft = body.content.into();
            profile.updated_at = layer2_core::now_rfc3339();
            profile.validate().map_err(invalid)?;
            validate_agent_references(
                &command_state,
                &owner,
                Some(&profile.id),
                &profile.draft.agent_profiles,
            )
            .await?;
            command_state
                .workspaces
                .save_workspace_draft(body.draft_version, profile.clone())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content.agent_profiles"))?;
            Ok(Json(view(&command_state, profile).await?).into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/workspace-profiles/{id}/revisions",
    tag = "workspace profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = PublishProfileRequest,
    responses((status = 201, body = WorkspaceProfileRevisionView))
)]
pub(super) async fn publish(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<PublishProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("workspace-profile:publish:{id}"),
        body,
        move |body| async move {
            let profile = owned(&command_state, &owner, &id).await?;
            validate_agent_references(
                &command_state,
                &owner,
                Some(&profile.id),
                &profile.draft.agent_profiles,
            )
            .await?;
            let revision = command_state
                .workspaces
                .publish_workspace_profile(
                    &profile.id,
                    body.draft_version,
                    layer2_core::now_rfc3339(),
                )
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content.agent_profiles"))?;
            Ok((
                axum::http::StatusCode::CREATED,
                Json(WorkspaceProfileRevisionView::from(revision)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace-profiles/{id}/revisions",
    tag = "workspace profiles",
    params(("id" = String, Path), ("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = WorkspaceProfileRevisionPageView))
)]
pub(super) async fn list_revisions(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<ProfilePageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<WorkspaceProfileRevisionPageView>, ProfileProblem> {
    authorized(&caller)?;
    let profile = owned(&state, &caller.subject, &id).await?;
    let page = profile_page(query.map_err(|_| ProfileProblem::InvalidPage)?.0)?;
    let stored = state
        .workspaces
        .list_workspace_profile_revisions(&profile.id, &page)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?;
    Ok(Json(WorkspaceProfileRevisionPageView {
        items: stored.items.into_iter().map(Into::into).collect(),
        next: stored.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace-profiles/{id}/revisions/{revision}",
    tag = "workspace profiles",
    params(("id" = String, Path), ("revision" = u32, Path)),
    responses((status = 200, body = WorkspaceProfileRevisionView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn read_revision(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, revision)): Path<(String, u32)>,
) -> Result<Json<WorkspaceProfileRevisionView>, ProfileProblem> {
    super::profiles::require_revision_read(&caller)?;
    if id != layer2_core::DEFAULT_WORKSPACE_PROFILE_ID && !generated_id_is_valid(&id, "wkp_") {
        return Err(ProfileProblem::NotFound);
    }
    let profile = state
        .workspaces
        .workspace_profile(&layer2_core::WorkspaceProfileId(id))
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .filter(|profile| {
            !profile.internal
                && super::profiles::revision_visible(
                    &caller,
                    &profile.owner,
                    profile.visibility,
                    profile.retired_at.is_some(),
                )
        })
        .ok_or(ProfileProblem::NotFound)?;
    let revision = state
        .workspaces
        .workspace_profile_revision(&profile.id, revision)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .ok_or(ProfileProblem::NotFound)?;
    Ok(Json(revision.into()))
}

#[utoipa::path(
    put,
    path = "/api/v1/workspace-profiles/{id}/visibility",
    tag = "workspace profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = SetProfileVisibilityRequest,
    responses((status = 200, body = WorkspaceProfileView), (status = 422, body = crate::rest::ProblemDocument))
)]
pub(super) async fn set_visibility(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<SetProfileVisibilityRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("workspace-profile:visibility:{id}"),
        body,
        move |body| async move {
            let profile = owned(&command_state, &owner, &id).await?;
            command_state
                .marketplace
                .set_profile_visibility(
                    &owner,
                    &layer2_core::profiles::MarketplaceLocator::Workspace(profile.id),
                    body.visibility.into(),
                    layer2_core::now_rfc3339(),
                    command_state.policy.capacity_bounds,
                    command_state.policy.public_value_scanner.as_ref(),
                )
                .await
                .map_err(|error| ProfileProblem::from_marketplace(error, true))?;
            Ok(
                Json(view(&command_state, owned(&command_state, &owner, &id).await?).await?)
                    .into_response(),
            )
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/workspace-profiles/{id}/retire",
    tag = "workspace profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = WorkspaceProfileView))
)]
pub(super) async fn retire(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_empty_command(
        &state,
        &caller,
        key,
        format!("workspace-profile:retire:{id}"),
        move || async move {
            let profile = owned(&command_state, &owner, &id).await?;
            command_state
                .workspaces
                .retire_workspace_profile(&profile.id, &owner, layer2_core::now_rfc3339())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content"))?;
            Ok(
                Json(view(&command_state, owned(&command_state, &owner, &id).await?).await?)
                    .into_response(),
            )
        },
    )
    .await
}
