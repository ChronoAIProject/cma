//! Shared composition and invariants for profile REST resources.

use std::{future::Future, sync::Arc};

use axum::{response::IntoResponse, response::Response, Router};
use layer2_core::profiles::{
    AgentProfileStore, MarketplaceStore, ProfileCapacityBounds, ProfilePageRequest,
    SandboxProfileStore, WorkspaceProfileStore,
};
use layer2_core::{Caller, Capability, OperationStore, PlatformSettingsService};

use super::idempotency::{self, IdempotencyKey, RequestBody as IdempotentBody};
use super::profiles_openapi_shapes::ProfilePageQuery;
use super::profiles_problem::ProfileProblem;
use super::BodyProblem;

/// Deployment policy needed while validating profile drafts and publication.
#[derive(Debug, Clone)]
pub struct ProfileRoutePolicy {
    pub capacity_bounds: ProfileCapacityBounds,
    pub public_value_scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
}

/// Provider-neutral ports used by the profile and marketplace routes.
#[derive(Clone)]
pub struct ProfilesState {
    pub(super) agents: Arc<dyn AgentProfileStore>,
    pub(super) workspaces: Arc<dyn WorkspaceProfileStore>,
    pub(super) sandboxes: Arc<dyn SandboxProfileStore>,
    pub(super) marketplace: Arc<dyn MarketplaceStore>,
    pub(super) operations: Arc<dyn OperationStore>,
    pub(super) settings: Arc<PlatformSettingsService>,
    pub(super) policy: ProfileRoutePolicy,
}

impl ProfilesState {
    pub fn new<S>(
        profiles: Arc<S>,
        operations: Arc<dyn OperationStore>,
        settings: Arc<PlatformSettingsService>,
        policy: ProfileRoutePolicy,
    ) -> Self
    where
        S: AgentProfileStore
            + WorkspaceProfileStore
            + SandboxProfileStore
            + MarketplaceStore
            + 'static,
    {
        Self {
            agents: profiles.clone(),
            workspaces: profiles.clone(),
            sandboxes: profiles.clone(),
            marketplace: profiles,
            operations,
            settings,
            policy,
        }
    }
}

pub(super) fn require_manage(caller: &Caller) -> Result<(), ProfileProblem> {
    caller
        .require(Capability::ManageOwnProfiles)
        .map_err(|_| ProfileProblem::Forbidden)
}

pub(super) fn generated_id_is_valid(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Inline Trigger targets use private immutable profile snapshots behind the
/// existing launch worker. They are implementation details and must never
/// appear in the user-facing profile library or be manageable by ID.
pub(super) fn internal_inline_profile(name: &str, description: &str) -> bool {
    name.starts_with("Inline Trigger ")
        && description.starts_with("Internal Agent Manager Trigger ")
}

pub(super) fn require_revision_read(caller: &Caller) -> Result<(), ProfileProblem> {
    if caller.authorization.allows(Capability::ManageOwnProfiles)
        || caller.authorization.allows(Capability::UseOwnSandboxes)
    {
        Ok(())
    } else {
        Err(ProfileProblem::Forbidden)
    }
}

pub(super) fn revision_visible(
    caller: &Caller,
    owner: &layer2_core::SubjectId,
    visibility: layer2_core::profiles::ProfileVisibility,
    retired: bool,
) -> bool {
    owner == &caller.subject
        || (!retired
            && caller.authorization.allows(Capability::UseOwnSandboxes)
            && visibility == layer2_core::profiles::ProfileVisibility::Public)
}

pub(super) fn profile_page(query: ProfilePageQuery) -> Result<ProfilePageRequest, ProfileProblem> {
    let limit = query.limit.unwrap_or(100);
    if limit == 0 || limit > 200 {
        return Err(ProfileProblem::InvalidPage);
    }
    Ok(ProfilePageRequest {
        limit,
        cursor: query.cursor,
    })
}

pub(super) async fn execute_command<T, F, Fut>(
    state: &ProfilesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    body: Result<IdempotentBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<Response, ProfileProblem>>,
{
    if let Err(problem) = require_manage(caller) {
        return problem.into_response();
    }
    let (guard, body, _) = match idempotency::required_command(
        key,
        Arc::clone(&state.operations),
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
    state: &ProfilesState,
    caller: &Caller,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response, ProfileProblem>>,
{
    if let Err(problem) = require_manage(caller) {
        return problem.into_response();
    }
    let guard = match idempotency::required_empty_command(
        key,
        Arc::clone(&state.operations),
        caller,
        operation,
    )
    .await
    {
        Ok(guard) => guard,
        Err(response) => return response,
    };
    let result = execute().await.map_err(IntoResponse::into_response);
    guard.finish(result).await
}

/// Compose all three own-profile surfaces and the authenticated marketplace.
pub fn profiles_router(state: ProfilesState) -> Router {
    super::profiles_agent::routes()
        .merge(super::profiles_workspace::routes())
        .merge(super::profiles_sandbox::routes())
        .merge(super::profiles_marketplace::routes())
        .route(
            "/api/v1/defaults",
            axum::routing::get(super::defaults::read),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::internal_inline_profile;

    #[test]
    fn only_inline_trigger_snapshots_are_hidden() {
        assert!(internal_inline_profile(
            "Inline Trigger Agent",
            "Internal Agent Manager Trigger configuration"
        ));
        assert!(internal_inline_profile(
            "Inline Trigger Sandbox",
            "Internal Agent Manager Trigger sandbox configuration"
        ));
        assert!(!internal_inline_profile(
            "Research Agent",
            "Internal Agent Manager Trigger configuration"
        ));
        assert!(!internal_inline_profile(
            "Inline Trigger Agent",
            "User supplied configuration"
        ));
    }
}
