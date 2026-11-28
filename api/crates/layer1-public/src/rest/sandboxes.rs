//! Owner-facing Sandbox routes.

pub(super) mod creation;
#[path = "sandbox_enrichment.rs"]
mod enrichment;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{OwnerSandboxQuery, SandboxStatus};
use layer2_core::{Capability, SandboxId};
use serde::Deserialize;

use super::hierarchy_shared_shapes::{
    AccessibleSandboxDetailView, AccessibleSandboxPageView, AccessibleSandboxSummaryView,
};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};
use layer2_core::sandbox::SandboxHierarchyReading;

use super::idempotency::{IdempotencyKey, RequestBody};
use super::instances::{execute_command, execute_empty_command, HierarchyState};
use super::instances_openapi_shapes::{OwnerSandboxSummaryView, SandboxEventView, SandboxView};
use super::instances_problem::HierarchyProblem;
use super::BodyProblem;

const DEFAULT_PAGE_LIMIT: u16 = 100;
const DEFAULT_EVENT_LIMIT: u16 = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SandboxPageParams {
    organization: Option<String>,
    #[serde(default)]
    shared: bool,
    status: Option<SandboxStatus>,
    #[serde(default)]
    exclude_retired: bool,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventParams {
    after: Option<u64>,
    limit: Option<u16>,
}

pub(super) fn routes() -> Router<HierarchyState> {
    Router::new()
        .route("/api/v1/sandboxes", post(creation::create).get(list))
        .route(
            "/api/v1/sandboxes/capacity",
            get(super::sandbox_capacity::read),
        )
        .route("/api/v1/sandboxes/{id}", get(read).delete(remove))
        .route("/api/v1/sandboxes/{id}/pause", post(pause))
        .route("/api/v1/sandboxes/{id}/revive", post(revive))
        .route(
            "/api/v1/sandboxes/{id}/always-on",
            post(super::sandbox_always_on::always_on),
        )
        .route(
            "/api/v1/sandboxes/{id}/runtime-update",
            post(super::sandbox_runtime_update::update),
        )
        .route(
            "/api/v1/sandboxes/{id}/runtime-recovery",
            get(super::sandbox_retained_recovery::inspect)
                .post(super::sandbox_retained_recovery::recover),
        )
        .route(
            "/api/v1/sandboxes/{id}/service-access",
            post(super::sandbox_service_access::grant),
        )
        .route("/api/v1/sandboxes/{id}/events", get(events))
}

#[utoipa::path(
    get,
    path = "/api/v1/sandboxes",
    tag = "sandboxes",
    params(
        ("organization" = Option<String>, Query),
        ("shared" = Option<bool>, Query, description = "Incoming whole-Sandbox grants across current organizations; mutually exclusive with organization"),
        ("status" = Option<String>, Query),
        ("exclude_retired" = Option<bool>, Query, description = "Exclude retired Sandboxes before pagination; omitted or false retains historical inventory"),
        ("cursor" = Option<String>, Query),
        ("limit" = Option<u16>, Query)
    ),
    responses((status = 200, body = AccessibleSandboxPageView))
)]
pub(super) async fn list(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    params: Result<Query<SandboxPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<AccessibleSandboxPageView>, HierarchyProblem> {
    let params = params.map_err(|_| HierarchyProblem::InvalidQuery)?.0;
    if params.cursor.as_ref().is_some_and(String::is_empty) {
        return Err(HierarchyProblem::InvalidPage);
    }
    let page = PageRequest::new(params.limit.unwrap_or(DEFAULT_PAGE_LIMIT), params.cursor)
        .map_err(|_| HierarchyProblem::InvalidPage)?;
    if params.shared && params.organization.is_some() {
        return Err(HierarchyProblem::InvalidQuery);
    }
    if params.shared {
        let resource = resource.ok_or(HierarchyProblem::Forbidden)?;
        let stored = state
            .hierarchy
            .shared_list_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &OwnerSandboxQuery {
                    exclude_retired: params.exclude_retired,
                    status: params.status,
                    page,
                },
            )
            .await
            .map_err(HierarchyProblem::sandbox_read)?;
        return Ok(Json(AccessibleSandboxPageView {
            items: stored.items.into_iter().map(Into::into).collect(),
            next: stored.next,
        }));
    }
    if let Some(organization) = params.organization {
        if organization.trim().is_empty()
            || organization.len() > 128
            || organization.trim() != organization
        {
            return Err(HierarchyProblem::InvalidQuery);
        }
        let resource = resource.ok_or(HierarchyProblem::Forbidden)?;
        // Resolve only the caller's own display name before acquiring inventory
        // evidence, so no directory IO can delay release past the final fence.
        let names =
            super::public_names::resolve(&state.users, std::slice::from_ref(&caller.subject)).await;
        let stored = state
            .hierarchy
            .organization_list_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &layer2_core::SubjectId::new(organization),
                &OwnerSandboxQuery {
                    exclude_retired: params.exclude_retired,
                    status: params.status,
                    page,
                },
            )
            .await
            .map_err(HierarchyProblem::sandbox_read)?;
        let mut items: Vec<_> = stored
            .items
            .into_iter()
            .map(|row| match row {
                layer2_core::sandbox::SandboxInventoryReading::Personal(value) => {
                    personal_summary(&state, &caller, *value)
                }
                shared => shared.into(),
            })
            .collect();
        for item in &mut items {
            if let AccessibleSandboxSummaryView::Personal(value) = item {
                value.sandbox.owner_name = names.get(&value.sandbox.owner).cloned();
            }
        }
        return Ok(Json(AccessibleSandboxPageView {
            items,
            next: stored.next,
        }));
    }
    let page = state
        .hierarchy
        .list_sandboxes(
            &caller,
            &OwnerSandboxQuery {
                exclude_retired: params.exclude_retired,
                status: params.status,
                page,
            },
        )
        .await
        .map_err(HierarchyProblem::sandbox_read)?;
    let subjects: Vec<_> = page
        .items
        .iter()
        .map(|row| row.sandbox.owner.clone())
        .collect();
    let names = super::public_names::resolve(&state.users, &subjects).await;
    let listed: Vec<_> = page.items.iter().map(|row| &row.sandbox).collect();
    let colors = super::sandbox_colors::colors(&state, &caller.subject, &listed).await;
    Ok(Json(AccessibleSandboxPageView {
        items: page
            .items
            .into_iter()
            .map(|row| {
                let mut summary = personal_summary(&state, &caller, row);
                if let AccessibleSandboxSummaryView::Personal(view) = &mut summary {
                    view.sandbox.owner_name = names.get(&view.sandbox.owner).cloned();
                    view.sandbox.color = colors.get(&view.sandbox.id).cloned();
                }
                summary
            })
            .collect(),
        next: page.next,
    }))
}

fn personal_summary(
    state: &HierarchyState,
    caller: &layer2_core::Caller,
    row: layer2_core::sandbox::SandboxHierarchySummary,
) -> AccessibleSandboxSummaryView {
    let result = state
        .sandboxes
        .rotation_lifecycle_result(caller, &row.sandbox);
    let mut view = OwnerSandboxSummaryView::from(row);
    if let Some(result) = result {
        view.sandbox.lifecycle.result = Some(result.into());
    }
    AccessibleSandboxSummaryView::Personal(Box::new(view))
}

#[utoipa::path(get, path = "/api/v1/sandboxes/{id}", tag = "sandboxes", params(("id" = String, Path)), responses((status = 200, body = AccessibleSandboxDetailView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<AccessibleSandboxDetailView>, HierarchyProblem> {
    let id = SandboxId(id);
    let reading = match resource {
        Some(resource) => {
            state
                .hierarchy
                .sandbox_authorized(
                    &resource.context,
                    resource.credential.as_ref().map(DelegatedCredential::value),
                    &id,
                )
                .await
        }
        None => state
            .hierarchy
            .sandbox(&caller, &id)
            .await
            .map(|value| SandboxHierarchyReading::Personal(Box::new(value))),
    }
    .map_err(HierarchyProblem::sandbox_read)?;
    let (color, owner_name) = enrichment::read(&state, &caller, &reading).await;
    let lifecycle_result = match &reading {
        SandboxHierarchyReading::Personal(personal) => state
            .sandboxes
            .lifecycle_result(&caller, &personal.sandbox)
            .await
            .map_err(HierarchyProblem::sandbox)?,
        _ => None,
    };
    let runtime_update = match (&reading, &state.runtime_updates) {
        (SandboxHierarchyReading::Personal(personal), Some(service))
            if personal.sandbox.owner == caller.subject =>
        {
            Some(
                super::sandbox_runtime_update::SandboxRuntimeUpdateView::new(
                    &personal.sandbox,
                    service,
                ),
            )
        }
        _ => None,
    };
    let mut view = AccessibleSandboxDetailView::from(reading);
    if let AccessibleSandboxDetailView::Personal(personal) = &mut view {
        personal.sandbox.lifecycle.result = lifecycle_result.map(Into::into);
        personal.sandbox.runtime_update = runtime_update;
        personal.sandbox.color = color;
        personal.sandbox.owner_name = owner_name;
    }
    Ok(Json(view))
}

#[utoipa::path(delete, path = "/api/v1/sandboxes/{id}", tag = "sandboxes", params(("id" = String, Path), ("Idempotency-Key" = String, Header)), responses((status = 202, body = SandboxView), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn remove(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        Capability::DeleteOwnSandbox,
        key,
        format!("sandbox:delete:{id}"),
        move || async move {
            let sandbox = command_state
                .sandboxes
                .delete(&command_caller, &SandboxId(id))
                .await
                .map_err(HierarchyProblem::sandbox)?;
            Ok((StatusCode::ACCEPTED, Json(SandboxView::from(&sandbox))).into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes/{id}/pause",
    tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses(
        (status = 202, description = "Sandbox pause scheduled", body = SandboxView),
        (status = 400, description = "missing_idempotency_key or invalid_idempotency_key", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "not_authenticated", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "forbidden", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "sandbox_not_found", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "sandbox_not_active, sandbox_lifecycle_busy, or idempotency operation conflict", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "idempotency_key_reused", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sandbox lifecycle unavailable", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn pause(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("sandbox:pause:{id}"),
        move || async move {
            let outcome = command_state
                .sandboxes
                .pause(&command_caller, &SandboxId(id))
                .await
                .map_err(HierarchyProblem::sandbox_lifecycle)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(SandboxView::from(&outcome.sandbox)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes/{id}/revive",
    tag = "sandboxes",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    responses(
        (status = 202, description = "Sandbox revival scheduled", body = SandboxView),
        (status = 400, description = "missing_idempotency_key or invalid_idempotency_key", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "not_authenticated", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "forbidden", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "sandbox_not_found", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "sandbox_not_paused, sandbox_lifecycle_busy, or idempotency operation conflict", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "idempotency_key_reused", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sandbox lifecycle unavailable", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn revive(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
) -> Response {
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_empty_command(
        &state,
        &caller,
        Capability::UseOwnSandboxes,
        key,
        format!("sandbox:revive:{id}"),
        move || async move {
            let outcome = command_state
                .sandboxes
                .revive(&command_caller, &SandboxId(id))
                .await
                .map_err(HierarchyProblem::sandbox_lifecycle)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(SandboxView::from(&outcome.sandbox)),
            )
                .into_response())
        },
    )
    .await
}

#[utoipa::path(get, path = "/api/v1/sandboxes/{id}/events", tag = "sandboxes", params(("id" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<u16>, Query)), responses((status = 200, body = [SandboxEventView]), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn events(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    params: Result<Query<EventParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Vec<SandboxEventView>>, HierarchyProblem> {
    let params = params.map_err(|_| HierarchyProblem::InvalidQuery)?.0;
    let limit = params.limit.unwrap_or(DEFAULT_EVENT_LIMIT);
    if !(1..=DEFAULT_EVENT_LIMIT).contains(&limit) {
        return Err(HierarchyProblem::InvalidPage);
    }
    state
        .sandboxes
        .events(&caller, &SandboxId(id), params.after.unwrap_or(0), limit)
        .await
        .map(|events| Json(events.into_iter().map(Into::into).collect()))
        .map_err(HierarchyProblem::sandbox)
}
