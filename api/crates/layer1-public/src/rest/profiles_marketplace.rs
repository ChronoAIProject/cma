//! Authenticated profile marketplace REST resource.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::profiles::{
    ForkedProfile, MarketplaceEntry, MarketplaceKind, MarketplaceLocator, MarketplaceQuery,
    MarketplaceRevision,
};
use layer2_core::{
    DEFAULT_AGENT_PROFILE_ID, DEFAULT_SANDBOX_PROFILE_ID, DEFAULT_WORKSPACE_PROFILE_ID,
};

use super::idempotency::{IdempotencyKey, RequestBody};
use super::profiles::{execute_command, generated_id_is_valid, ProfilesState};
use super::profiles_openapi_shapes::*;
use super::profiles_problem::ProfileProblem;
use super::BodyProblem;
use crate::AuthenticatedCaller;

pub(super) fn routes() -> Router<ProfilesState> {
    Router::new()
        .route("/api/v1/marketplace/{kind}", get(list))
        .route("/api/v1/marketplace/{kind}/{id}", get(read))
        .route("/api/v1/marketplace/{kind}/{id}/fork", post(fork))
}

async fn require_enabled(state: &ProfilesState) -> Result<(), ProfileProblem> {
    state
        .settings
        .current()
        .await
        .map_err(|_| ProfileProblem::StoreUnavailable)?
        .settings
        .marketplace
        .enabled
        .then_some(())
        .ok_or(ProfileProblem::MarketplaceDisabled)
}

fn kind(value: &str) -> Result<MarketplaceKind, ProfileProblem> {
    match value {
        "agent-profiles" => Ok(MarketplaceKind::Agent),
        "workspace-profiles" => Ok(MarketplaceKind::Workspace),
        "sandbox-profiles" => Ok(MarketplaceKind::Sandbox),
        _ => Err(ProfileProblem::NotFound),
    }
}

fn locator(kind: &str, id: String) -> Result<MarketplaceLocator, ProfileProblem> {
    let expected = self::kind(kind)?;
    let (prefix, reserved, locator) = match expected {
        MarketplaceKind::Agent => (
            "agp_",
            DEFAULT_AGENT_PROFILE_ID,
            MarketplaceLocator::Agent(layer2_core::AgentProfileId(id.clone())),
        ),
        MarketplaceKind::Workspace => (
            "wkp_",
            DEFAULT_WORKSPACE_PROFILE_ID,
            MarketplaceLocator::Workspace(layer2_core::WorkspaceProfileId(id.clone())),
        ),
        MarketplaceKind::Sandbox => (
            "sbp_",
            DEFAULT_SANDBOX_PROFILE_ID,
            MarketplaceLocator::Sandbox(layer2_core::SandboxProfileId(id.clone())),
        ),
    };
    (id == reserved || generated_id_is_valid(&id, prefix))
        .then_some(locator)
        .ok_or(ProfileProblem::NotFound)
}

#[cfg(test)]
mod identifier_tests {
    use super::*;

    #[test]
    fn only_the_kind_matched_reserved_identifier_bypasses_generated_id_validation() {
        assert!(locator("agent-profiles", DEFAULT_AGENT_PROFILE_ID.into()).is_ok());
        assert!(locator("workspace-profiles", DEFAULT_WORKSPACE_PROFILE_ID.into()).is_ok());
        assert!(locator("sandbox-profiles", DEFAULT_SANDBOX_PROFILE_ID.into()).is_ok());
        assert!(locator("agent-profiles", DEFAULT_WORKSPACE_PROFILE_ID.into()).is_err());
        assert!(locator("agent-profiles", "agp_other".into()).is_err());
    }
}

fn entry_view(entry: MarketplaceEntry, consumer: &layer2_core::SubjectId) -> MarketplaceEntryView {
    let nested_workspace_requires_confirmation = entry.resolved_revisions.iter().any(|resolved| {
        resolved.owner != *consumer
            && matches!(
                &resolved.revision,
                MarketplaceRevision::Workspace(revision)
                    if !revision.content.setup_commands.is_empty()
            )
    });
    let summary = entry.summary.into();
    let resolved_profiles = entry
        .resolved_profiles
        .into_iter()
        .map(Into::into)
        .collect();
    let resolved_revisions = entry
        .resolved_revisions
        .into_iter()
        .map(Into::into)
        .collect();
    match entry.revision {
        MarketplaceRevision::Agent(revision) => MarketplaceEntryView::Agent {
            summary,
            revision: revision.into(),
            resolved_profiles,
            resolved_revisions,
        },
        MarketplaceRevision::Workspace(revision) => MarketplaceEntryView::Workspace {
            summary,
            revision: revision.into(),
            resolved_profiles,
            resolved_revisions,
        },
        MarketplaceRevision::Sandbox(revision) => {
            let has_commands_or_environment =
                !revision.content.software_install_commands.is_empty()
                    || !revision.content.environment_variables.is_empty();
            let trust = SandboxProfileTrustView {
                requires_confirmation: (entry.owner != *consumer && has_commands_or_environment)
                    || nested_workspace_requires_confirmation,
                commands_digest: revision.digest.clone(),
            };
            MarketplaceEntryView::Sandbox {
                summary,
                revision: revision.into(),
                resolved_profiles,
                resolved_revisions,
                trust,
            }
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/marketplace/{kind}",
    tag = "profile marketplace",
    params(("kind" = String, Path), ("q" = Option<String>, Query), ("cursor" = Option<String>, Query), ("limit" = Option<u16>, Query)),
    responses((status = 200, body = MarketplacePageView))
)]
pub(super) async fn list(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(_caller): AuthenticatedCaller,
    Path(kind_value): Path<String>,
    query: Result<Query<MarketplacePageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<MarketplacePageView>, ProfileProblem> {
    require_enabled(&state).await?;
    let query = query.map_err(|_| ProfileProblem::InvalidPage)?.0;
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 50 {
        return Err(ProfileProblem::InvalidPage);
    }
    let page = state
        .marketplace
        .list_marketplace(&MarketplaceQuery {
            kind: Some(kind(&kind_value)?),
            text: query.q,
            cursor: query.cursor,
            limit,
        })
        .await
        .map_err(|error| ProfileProblem::from_marketplace(error, false))?;
    Ok(Json(MarketplacePageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/marketplace/{kind}/{id}",
    tag = "profile marketplace",
    params(("kind" = String, Path), ("id" = String, Path)),
    responses((status = 200, body = MarketplaceEntryView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn read(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((kind_value, id)): Path<(String, String)>,
) -> Result<Json<MarketplaceEntryView>, ProfileProblem> {
    require_enabled(&state).await?;
    let entry = state
        .marketplace
        .read_marketplace(&locator(&kind_value, id)?)
        .await
        .map_err(|error| ProfileProblem::from_marketplace(error, false))?;
    Ok(Json(entry_view(entry, &caller.subject)))
}

#[utoipa::path(
    post,
    path = "/api/v1/marketplace/{kind}/{id}/fork",
    tag = "profile marketplace",
    params(("kind" = String, Path), ("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ForkProfileRequest,
    responses((status = 201, body = ForkedProfileView), (status = 404, body = crate::rest::ProblemDocument))
)]
pub(super) async fn fork(
    State(state): State<ProfilesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((kind_value, id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<ForkProfileRequest>, BodyProblem>,
) -> Response {
    let command_state = state.clone();
    let owner = caller.subject.clone();
    execute_command(
        &state,
        &caller,
        key,
        format!("marketplace:fork:{kind_value}:{id}"),
        body,
        move |body| async move {
            require_enabled(&command_state).await?;
            let locator = locator(&kind_value, id)?;
            let revision = match body.revision {
                Some(revision) if revision > 0 => revision,
                Some(_) => return Err(ProfileProblem::NotFound),
                None => {
                    command_state
                        .marketplace
                        .read_marketplace(&locator)
                        .await
                        .map_err(|error| ProfileProblem::from_marketplace(error, false))?
                        .summary
                        .latest_revision
                }
            };
            let forked = command_state
                .marketplace
                .fork_profile(&owner, &locator, revision, layer2_core::now_rfc3339())
                .await
                .map_err(|error| ProfileProblem::from_marketplace(error, false))?;
            let view = match forked {
                ForkedProfile::Agent(profile) => {
                    ForkedProfileView::Agent(AgentProfileView::new(profile, None))
                }
                ForkedProfile::Workspace(profile) => {
                    ForkedProfileView::Workspace(WorkspaceProfileView::new(profile, None))
                }
                ForkedProfile::Sandbox(profile) => {
                    ForkedProfileView::Sandbox(SandboxProfileView::new(profile, None))
                }
            };
            Ok((axum::http::StatusCode::CREATED, Json(view)).into_response())
        },
    )
    .await
}
