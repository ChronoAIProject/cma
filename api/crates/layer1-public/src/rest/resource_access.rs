//! Owner share management and current effective access. These handlers never
//! accept an owner, actor, membership observation or authorization fence.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::resource_access::{
    AccessPurpose, ResourceAccessService, ResourceAccessStoreError, ResourceCapability,
    ResourceSelector, ResourceTarget, ShareChange, ShareCommand,
};
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use std::sync::Arc;

use super::{
    idempotency::{IdempotencyKey, RequestBody},
    resource_access_problem::ResourceProblem,
    resource_access_shapes::*,
    BodyProblem,
};
use crate::AuthenticatedResourceCaller;

pub fn resource_access_router(service: Arc<ResourceAccessService>) -> Router {
    Router::new()
        .merge(super::organizations::routes())
        .merge(super::resource_revocations::routes())
        .merge(super::agent_share_links::routes())
        .route("/api/v1/resource-access/{kind}/{id}", get(effective))
        .route(
            "/api/v1/resource-shares/{kind}/{id}",
            get(list).post(create),
        )
        .route(
            "/api/v1/resource-shares/{kind}/{id}/{share_id}",
            get(read).patch(update).delete(revoke),
        )
        .with_state(service)
}

fn target(kind: &str, id: String) -> Result<ResourceTarget, ResourceProblem> {
    if id.trim().is_empty() || id.len() > 128 {
        return Err(invalid());
    }
    match kind {
        "agent" => Ok(ResourceTarget::Agent {
            agent_id: AgentId(id),
        }),
        "sandbox" => Ok(ResourceTarget::Sandbox {
            sandbox_id: SandboxId(id),
        }),
        "workspace" => Ok(ResourceTarget::Workspace {
            workspace_id: WorkspaceId(id),
        }),
        "worktree" => Ok(ResourceTarget::Worktree {
            worktree_id: WorktreeId(id),
        }),
        _ => Err(invalid()),
    }
}
pub(super) fn selector(kind: &str, id: String) -> Result<ResourceSelector, ResourceProblem> {
    match target(kind, id)? {
        ResourceTarget::Agent { agent_id } => Ok(ResourceSelector::Agent { agent_id }),
        ResourceTarget::Sandbox { sandbox_id } => Ok(ResourceSelector::Sandbox { sandbox_id }),
        _ => Err(invalid()),
    }
}
fn invalid() -> ResourceProblem {
    ResourceProblem(ResourceAccessStoreError::Invalid.into())
}

#[utoipa::path(get, path = "/api/v1/resource-access/{kind}/{id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path)),
    responses((status = 200, body = EffectiveAccessView), (status = 404, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn effective(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<EffectiveAccessView>, ResourceProblem> {
    let target = target(&kind, id)?;
    let authorized = service
        .authorize(
            &caller.context,
            caller
                .credential
                .as_ref()
                .map(|credential| credential.value()),
            &target,
            ResourceCapability::ViewDetails,
            AccessPurpose::Read,
        )
        .await?;
    use ResourceCapability::*;
    let capabilities = authorized
        .decision
        .capabilities()
        .map(|capability| {
            match capability {
                ViewDetails => "view_details",
                ViewHistory => "view_history",
                ViewArtifacts => "view_artifacts",
                ViewFiles => "view_files",
                Interact => "interact",
                ResolveApproval => "resolve_approval",
                OperateAgent => "operate_agent",
                ManageSandbox => "manage_sandbox",
                ManageBridge => "manage_bridge",
                ManageSharing => "manage_sharing",
            }
            .to_owned()
        })
        .collect();
    let shared_by = if !authorized
        .decision
        .allows(ResourceCapability::ManageSharing)
    {
        if let Some(share) = authorized.snapshot.shares.iter().find(|share| {
            share.link.as_ref().is_some_and(|link| {
                link.accepted_subjects
                    .contains(&caller.context.caller.subject)
            })
        }) {
            Some(service.share_owner_identity(share).await?.into())
        } else {
            None
        }
    } else {
        None
    };
    Ok(Json(EffectiveAccessView {
        shared_by,
        owner: match authorized
            .snapshot
            .scope(&target)
            .map_err(|error| ResourceProblem(error.into()))?
            .owner
        {
            layer2_core::resource_access::ResourceOwner::Person { subject } => {
                ResourceOwnershipView::Person {
                    is_current_person: subject == caller.context.caller.subject,
                }
            }
            layer2_core::resource_access::ResourceOwner::Organization { subject } => {
                ResourceOwnershipView::Organization {
                    organization_id: subject.to_string(),
                }
            }
        },
        capabilities,
        revalidate_at: authorized
            .decision
            .revalidate_at
            .map(layer2_core::rfc3339_at),
        policy_revision: authorized.snapshot.fence().policy_revision,
    }))
}

#[utoipa::path(get, path = "/api/v1/resource-shares/{kind}/{id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path)), responses((status = 200, body = ActiveShareList)))]
pub(crate) async fn list(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<ActiveShareList>, ResourceProblem> {
    let items = service
        .active_shares(
            &caller.context,
            caller
                .credential
                .as_ref()
                .map(|credential| credential.value()),
            &selector(&kind, id)?,
        )
        .await?;
    Ok(Json(ActiveShareList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/resource-shares/{kind}/{id}/{share_id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path), ("share_id" = String, Path)), responses((status = 200, body = ResourceShareView)))]
pub(crate) async fn read(
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id, share_id)): Path<(String, String, String)>,
) -> Result<Json<ResourceShareView>, ResourceProblem> {
    Ok(Json(
        service
            .read_share(
                &caller.context,
                caller
                    .credential
                    .as_ref()
                    .map(|credential| credential.value()),
                &selector(&kind, id)?,
                &share_id,
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(post, path = "/api/v1/resource-shares/{kind}/{id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = CreateShareBody,
    responses((status = 200, body = ShareReceiptView), (status = 409, body = super::ProblemDocument)))]
pub(crate) async fn create(
    revocations: Option<super::resource_revocations::Revocations>,
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateShareBody>, BodyProblem>,
) -> Response {
    let body = match decode(body) {
        Ok(body) => body,
        Err(problem) => return problem.into_response(),
    };
    change(
        revocations,
        service,
        caller,
        (kind, id),
        key,
        ShareChange::Create {
            target_org_id: SubjectId::new(body.target_org_id),
            mode: body.mode.into(),
        },
        None,
    )
    .await
}

#[utoipa::path(patch, path = "/api/v1/resource-shares/{kind}/{id}/{share_id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path), ("share_id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = UpdateShareBody,
    responses((status = 200, body = super::resource_revocations::RevocationAwareSuccess<ShareReceiptView>), (status = 409, body = super::ProblemDocument)))]
pub(crate) async fn update(
    revocations: Option<super::resource_revocations::Revocations>,
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id, share_id)): Path<(String, String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateShareBody>, BodyProblem>,
) -> Response {
    let body = match decode(body) {
        Ok(body) => body,
        Err(problem) => return problem.into_response(),
    };
    change(
        revocations,
        service,
        caller,
        (kind, id),
        key,
        ShareChange::SetMode {
            share_id,
            mode: body.mode.into(),
        },
        Some(body.expected_revision),
    )
    .await
}

#[utoipa::path(delete, path = "/api/v1/resource-shares/{kind}/{id}/{share_id}", tag = "resource-access",
    params(("kind" = String, Path), ("id" = String, Path), ("share_id" = String, Path), ("Idempotency-Key" = String, Header), ("expected_revision" = u64, Query)),
    responses((status = 200, body = super::resource_revocations::RevocationAwareSuccess<ShareReceiptView>), (status = 409, body = super::ProblemDocument)))]
pub(crate) async fn revoke(
    revocations: Option<super::resource_revocations::Revocations>,
    State(service): State<Arc<ResourceAccessService>>,
    caller: AuthenticatedResourceCaller,
    Path((kind, id, share_id)): Path<(String, String, String)>,
    key: IdempotencyKey,
    query: Result<Query<RevokeShareQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let query = match query {
        Ok(Query(query)) => query,
        Err(_) => return invalid().into_response(),
    };
    change(
        revocations,
        service,
        caller,
        (kind, id),
        key,
        ShareChange::Revoke { share_id },
        Some(query.expected_revision),
    )
    .await
}

fn decode<T>(body: Result<RequestBody<T>, BodyProblem>) -> Result<T, BodyProblem> {
    body.and_then(|body| body.into_parts().0)
}
async fn change(
    revocations: Option<super::resource_revocations::Revocations>,
    service: Arc<ResourceAccessService>,
    caller: AuthenticatedResourceCaller,
    (kind, id): (String, String),
    key: IdempotencyKey,
    change: ShareChange,
    expected_share_revision: Option<u64>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(problem) => return problem.into_response(),
    };
    let selector = match selector(&kind, id) {
        Ok(selector) => selector,
        Err(problem) => return problem.into_response(),
    };
    let command = ShareCommand {
        selector,
        change,
        expected_share_revision,
        idempotency_key: key,
    };
    if matches!(
        command.change,
        ShareChange::Revoke { .. } | ShareChange::SetMode { .. }
    ) {
        if let Some(axum::Extension(revocations)) = revocations {
            return match revocations
                .request(
                    &caller.context,
                    caller.credential.as_ref().map(|c| c.value()),
                    command,
                )
                .await
            {
                Ok(value) => match value.receipt.clone() {
                    Some(receipt) => super::resource_revocations::completed(
                        &value,
                        ShareReceiptView::from(receipt),
                    ),
                    None => super::resource_revocations::pending(&value),
                },
                Err(error) => super::resource_revocations::problem(error),
            };
        }
    }
    match service
        .change_share(
            &caller.context,
            caller.credential.as_ref().map(|c| c.value()),
            command,
        )
        .await
    {
        Ok(receipt) => Json(ShareReceiptView::from(receipt)).into_response(),
        Err(error) => ResourceProblem(error).into_response(),
    }
}
