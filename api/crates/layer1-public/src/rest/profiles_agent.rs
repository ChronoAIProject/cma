//! Agent Profile REST resource.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::{
    AgentProfile, ProfilePageRequest, ProfileStoreError, ProfileVisibility,
};
use layer2_core::{Capability, SubjectId};

use super::idempotency::{IdempotencyKey, RequestBody};
use super::profiles::{
    execute_command, execute_empty_command, generated_id_is_valid, internal_inline_profile,
    profile_page, ProfilesState,
};
use super::profiles_openapi_shapes::*;
use super::profiles_problem::ProfileProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;

pub(super) fn routes() -> Router<ProfilesState> {
    Router::new()
        .route("/api/v1/agent-profiles", post(create).get(list))
        .route("/api/v1/agent-profiles/{id}", get(read).put(update))
        .route(
            "/api/v1/agent-profiles/{id}/revisions",
            post(publish).get(list_revisions),
        )
        .route(
            "/api/v1/agent-profiles/{id}/revisions/{revision}",
            get(read_revision),
        )
        .route(
            "/api/v1/agent-profiles/{id}/visibility",
            axum::routing::put(set_visibility),
        )
        .route("/api/v1/agent-profiles/{id}/retire", post(retire))
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
) -> Result<AgentProfile, ProfileProblem> {
    if !generated_id_is_valid(id, "agp_") {
        return Err(ProfileProblem::NotFound);
    }
    state
        .agents
        .agent_profile(&layer2_core::AgentProfileId(id.to_owned()))
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .filter(|profile| {
            &profile.owner == owner && !internal_inline_profile(&profile.name, &profile.description)
        })
        .ok_or(ProfileProblem::NotFound)
}

async fn latest(
    state: &ProfilesState,
    profile: &AgentProfile,
) -> Result<Option<ProfileRevisionSummary>, ProfileProblem> {
    let page = state
        .agents
        .list_agent_profile_revisions(
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
        .map(AgentProfileRevisionView::from)
        .as_ref()
        .map(ProfileRevisionSummary::from))
}

async fn view(
    state: &ProfilesState,
    profile: AgentProfile,
) -> Result<AgentProfileView, ProfileProblem> {
    let revision = latest(state, &profile).await?;
    Ok(AgentProfileView::new(profile, revision))
}

fn invalid(error: layer2_core::profiles::ProfileError) -> ProfileProblem {
    ProfileProblem::from_store(ProfileStoreError::Validation(error), "content")
}

#[utoipa::path(
    post,
    path = "/api/v1/agent-profiles",
    tag = "agent profiles",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateAgentProfileRequest,
    responses((status = 201, body = AgentProfileView), (status = 422, body = crate::rest::ProblemDocument))
)]
pub(super) async fn create(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateAgentProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        "agent-profile:create".into(),
        body,
        move |body| async move {
            let now = layer2_core::now_rfc3339();
            let profile = AgentProfile {
                id: layer2_core::AgentProfileId::generate(),
                owner,
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
            command_state
                .agents
                .create_agent_profile(profile.clone())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content"))?;
            Ok((
                axum::http::StatusCode::CREATED,
                Json(AgentProfileView::new(profile, None)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/agent-profiles",
    tag = "agent profiles",
    params(("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = AgentProfilePageView))
)]
pub(super) async fn list(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<ProfilePageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AgentProfilePageView>, ProfileProblem> {
    authorized(&caller)?;
    let page = profile_page(query.map_err(|_| ProfileProblem::InvalidPage)?.0)?;
    let stored = state
        .agents
        .list_agent_profiles(Some(&caller.subject), false, &page)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?;
    let mut items = Vec::with_capacity(stored.items.len());
    for profile in stored.items {
        if !internal_inline_profile(&profile.name, &profile.description) {
            items.push(view(&state, profile).await?);
        }
    }
    Ok(Json(AgentProfilePageView {
        items,
        next: stored.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agent-profiles/{id}",
    tag = "agent profiles",
    params(("id" = String, Path)),
    responses((status = 200, body = AgentProfileView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn read(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<AgentProfileView>, ProfileProblem> {
    authorized(&caller)?;
    Ok(Json(
        view(&state, owned(&state, &caller.subject, &id).await?).await?,
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/agent-profiles/{id}",
    tag = "agent profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateAgentProfileRequest,
    responses((status = 200, body = AgentProfileView), (status = 409, body = crate::rest::ProblemDocument))
)]
pub(super) async fn update(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateAgentProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("agent-profile:update:{id}"),
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
            command_state
                .agents
                .save_agent_draft(body.draft_version, profile.clone())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content"))?;
            Ok(Json(view(&command_state, profile).await?).into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/agent-profiles/{id}/revisions",
    tag = "agent profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = PublishProfileRequest,
    responses((status = 201, body = AgentProfileRevisionView))
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
        format!("agent-profile:publish:{id}"),
        body,
        move |body| async move {
            let profile = owned(&command_state, &owner, &id).await?;
            let revision = command_state
                .agents
                .publish_agent_profile(&profile.id, body.draft_version, layer2_core::now_rfc3339())
                .await
                .map_err(|error| ProfileProblem::from_store(error, "content"))?;
            Ok((
                axum::http::StatusCode::CREATED,
                Json(AgentProfileRevisionView::from(revision)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/agent-profiles/{id}/revisions",
    tag = "agent profiles",
    params(("id" = String, Path), ("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = AgentProfileRevisionPageView))
)]
pub(super) async fn list_revisions(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<ProfilePageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AgentProfileRevisionPageView>, ProfileProblem> {
    authorized(&caller)?;
    let profile = owned(&state, &caller.subject, &id).await?;
    let page = profile_page(query.map_err(|_| ProfileProblem::InvalidPage)?.0)?;
    let stored = state
        .agents
        .list_agent_profile_revisions(&profile.id, &page)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?;
    Ok(Json(AgentProfileRevisionPageView {
        items: stored.items.into_iter().map(Into::into).collect(),
        next: stored.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/agent-profiles/{id}/revisions/{revision}",
    tag = "agent profiles",
    params(("id" = String, Path), ("revision" = u32, Path)),
    responses((status = 200, body = AgentProfileRevisionView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn read_revision(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, revision)): Path<(String, u32)>,
) -> Result<Json<AgentProfileRevisionView>, ProfileProblem> {
    super::profiles::require_revision_read(&caller)?;
    if id != layer2_core::DEFAULT_AGENT_PROFILE_ID && !generated_id_is_valid(&id, "agp_") {
        return Err(ProfileProblem::NotFound);
    }
    let profile = state
        .agents
        .agent_profile(&layer2_core::AgentProfileId(id))
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .filter(|profile| {
            !internal_inline_profile(&profile.name, &profile.description)
                && super::profiles::revision_visible(
                    &caller,
                    &profile.owner,
                    profile.visibility,
                    profile.retired_at.is_some(),
                )
        })
        .ok_or(ProfileProblem::NotFound)?;
    let revision = state
        .agents
        .agent_profile_revision(&profile.id, revision)
        .await
        .map_err(|error| ProfileProblem::from_store(error, "content"))?
        .ok_or(ProfileProblem::NotFound)?;
    Ok(Json(revision.into()))
}

#[utoipa::path(
    put,
    path = "/api/v1/agent-profiles/{id}/visibility",
    tag = "agent profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = SetProfileVisibilityRequest,
    responses((status = 200, body = AgentProfileView), (status = 422, body = crate::rest::ProblemDocument))
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
        format!("agent-profile:visibility:{id}"),
        body,
        move |body| async move {
            let profile = owned(&command_state, &owner, &id).await?;
            command_state
                .marketplace
                .set_profile_visibility(
                    &owner,
                    &layer2_core::profiles::MarketplaceLocator::Agent(profile.id.clone()),
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
    path = "/api/v1/agent-profiles/{id}/retire",
    tag = "agent profiles",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = AgentProfileView))
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
        format!("agent-profile:retire:{id}"),
        move || async move {
            let profile = owned(&command_state, &owner, &id).await?;
            command_state
                .agents
                .retire_agent_profile(&profile.id, &owner, layer2_core::now_rfc3339())
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
