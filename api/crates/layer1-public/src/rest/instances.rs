//! Shared composition for owner-facing hierarchy resources.

use std::{future::Future, sync::Arc};

use axum::{response::IntoResponse, response::Response, Router};
use layer2_core::sandbox::{HierarchyReadService, SandboxService, WorkspaceService};
use layer2_core::{Caller, Capability, OperationStore};

use super::idempotency::{self, IdempotencyKey, RequestBody};
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;

#[derive(Clone)]
pub struct HierarchyState {
    pub(super) colors: Option<Arc<layer2_core::sandbox::colors::SandboxColorService>>,
    pub(super) runtime_updates: Option<Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>>,
    pub(super) retained_recovery: Option<Arc<layer2_core::sandbox::RetainedRecoveryService>>,
    pub(super) retained_reopening: Option<Arc<layer2_core::sandbox::RetainedReopenService>>,
    pub(super) users: Option<Arc<layer2_core::UserDirectory>>,
    pub(super) sandboxes: Arc<SandboxService>,
    pub(super) workspaces: Arc<WorkspaceService>,
    pub(super) hierarchy: Arc<HierarchyReadService>,
    pub(super) operations: Arc<dyn OperationStore>,
}

impl HierarchyState {
    pub fn new(
        sandboxes: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        hierarchy: Arc<HierarchyReadService>,
        operations: Arc<dyn OperationStore>,
    ) -> Self {
        Self {
            colors: None,
            users: None,
            runtime_updates: None,
            retained_recovery: None,
            retained_reopening: None,
            sandboxes,
            workspaces,
            hierarchy,
            operations,
        }
    }

    pub fn with_runtime_updates(
        mut self,
        service: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    ) -> Self {
        self.runtime_updates = Some(service);
        self
    }

    pub fn with_retained_recovery(
        mut self,
        service: Arc<layer2_core::sandbox::RetainedRecoveryService>,
    ) -> Self {
        self.retained_recovery = Some(service);
        self
    }

    pub fn with_retained_reopening(
        mut self,
        service: Arc<layer2_core::sandbox::RetainedReopenService>,
    ) -> Self {
        self.retained_reopening = Some(service);
        self
    }

    pub fn with_user_directory(mut self, users: Arc<layer2_core::UserDirectory>) -> Self {
        self.users = Some(users);
        self
    }

    pub fn with_sandbox_colors(
        mut self,
        colors: Arc<layer2_core::sandbox::colors::SandboxColorService>,
    ) -> Self {
        self.colors = Some(colors);
        self
    }
}

pub(super) async fn execute_command<T, F, Fut>(
    state: &HierarchyState,
    caller: &Caller,
    capability: Capability,
    key: IdempotencyKey,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
    execute: F,
) -> Response
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(T) -> Fut,
    Fut: Future<Output = Result<Response, HierarchyProblem>>,
{
    if !caller.authorization.allows(capability) {
        return HierarchyProblem::Forbidden.into_response();
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
    state: &HierarchyState,
    caller: &Caller,
    capability: Capability,
    key: IdempotencyKey,
    operation: String,
    execute: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Response, HierarchyProblem>>,
{
    if !caller.authorization.allows(capability) {
        return HierarchyProblem::Forbidden.into_response();
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

pub(super) fn confirmed_digest(
    trust_confirmed: bool,
    commands_digest: Option<String>,
) -> Result<Option<String>, HierarchyProblem> {
    match (trust_confirmed, commands_digest) {
        (false, None) => Ok(None),
        (true, Some(digest))
            if digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            Ok(Some(digest))
        }
        _ => Err(HierarchyProblem::Invalid("invalid_trust_confirmation")),
    }
}

/// Compose the owner-facing Sandbox, Workspace, and Worktree hierarchy.
pub fn hierarchy_router(state: HierarchyState) -> Router {
    super::sandboxes::routes()
        .merge(super::workspaces::routes())
        .merge(super::worktrees::routes())
        .with_state(state)
}
