//! Authenticated public projections of durable Agent service-access decisions.
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use layer2_core::{
    sandbox::agent_access::{
        validate_access_services, AgentAccessError, AgentAccessRequest, AgentAccessRequests,
        AgentAccessStatus, GithubConnectionApproval,
    },
    AgentId, SandboxId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    idempotency::{IdempotencyKey, RequestBody},
    BodyProblem,
};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller};

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentAccessStatusView {
    Pending,
    Connecting,
    Applying,
    Succeeded,
    Denied,
    Failed,
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AgentAccessConnectionTarget {
    AgentAccessRequest { id: String },
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentAccessConnection {
    pub target: AgentAccessConnectionTarget,
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentAccessRequestView {
    pub id: String,
    pub agent_id: String,
    pub status: AgentAccessStatusView,
    pub services: Vec<String>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<AgentAccessConnection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_approval: Option<GithubApprovalContextView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GithubApprovalContextView {
    sandbox_id: String,
    sandbox_name: String,
    other_agents: Vec<String>,
    current_user_service_id: Option<String>,
    scope_revision: u64,
    credential_generation: u64,
}
impl From<AgentAccessRequest> for AgentAccessRequestView {
    fn from(value: AgentAccessRequest) -> Self {
        let connection = matches!(
            value.status,
            AgentAccessStatus::Connecting | AgentAccessStatus::Applying
        )
        .then(|| AgentAccessConnection {
            target: AgentAccessConnectionTarget::AgentAccessRequest {
                id: value.id.clone(),
            },
        });
        Self {
            id: value.id,
            agent_id: value.agent_id.to_string(),
            status: match value.status {
                AgentAccessStatus::Pending => AgentAccessStatusView::Pending,
                AgentAccessStatus::Connecting => AgentAccessStatusView::Connecting,
                AgentAccessStatus::Applying => AgentAccessStatusView::Applying,
                AgentAccessStatus::Succeeded => AgentAccessStatusView::Succeeded,
                AgentAccessStatus::Denied => AgentAccessStatusView::Denied,
                AgentAccessStatus::Failed => AgentAccessStatusView::Failed,
            },
            services: value.services,
            reason: value.reason,
            retry_of: value.retry_of,
            connection,
            error: value.error,
            github_approval: value
                .github_approval
                .map(|approval| GithubApprovalContextView {
                    sandbox_id: approval.sandbox_id.to_string(),
                    sandbox_name: approval.sandbox_name,
                    other_agents: approval
                        .other_agents
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect(),
                    current_user_service_id: approval.current_user_service_id,
                    scope_revision: approval.scope_revision,
                    credential_generation: approval.credential_generation,
                }),
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentAccessRequestList {
    pub items: Vec<AgentAccessRequestView>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApproveAgentAccessRequest {
    #[schema(min_items = 1, max_items = 32)]
    pub services: Vec<String>,
    pub github_connection: Option<GithubConnectionApprovalBody>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GithubConnectionApprovalBody {
    sandbox_id: String,
    user_service_id: String,
    scope_revision: u64,
    credential_generation: u64,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DenyAgentAccessRequest {}

pub(super) mod scope;

/// Optional composition surface; the injected core service owns authority and receipts.
pub fn agent_access_requests_router(service: Arc<dyn AgentAccessRequests>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/service-access", put(scope::replace))
        .route("/api/v1/agents/{id}/access-requests", get(list))
        .route(
            "/api/v1/agents/{id}/access-requests/{request_id}/approve",
            post(approve),
        )
        .route(
            "/api/v1/agents/{id}/access-requests/{request_id}/deny",
            post(deny),
        )
        .layer(DefaultBodyLimit::max(8192))
        .layer(axum::middleware::from_fn(private_headers))
        .with_state(service)
}
async fn private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[utoipa::path(get, operation_id = "get_api_v1_agents_by_id_access_requests", path = "/api/v1/agents/{id}/access-requests", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = AgentAccessRequestList),
    (status = 400, body = super::ProblemDocument), (status = 401, body = super::ProblemDocument),
    (status = 403, body = super::ProblemDocument), (status = 404, body = super::ProblemDocument),
    (status = 409, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn list(
    State(service): State<Arc<dyn AgentAccessRequests>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match service.list(&caller, &AgentId(id)).await {
        Ok(items) => Json(AgentAccessRequestList {
            items: items.into_iter().map(Into::into).collect(),
        })
        .into_response(),
        Err(error) => problem(error),
    }
}

/// Approve with a direct owner login or verified owner access token. Independent
/// clients require existing service/backing-resource authority. Runtime keys and
/// delegated actors cannot approve; missing authority and connections return
/// `agent_access_owner_authority_required` and `agent_access_service_connection_required`.
#[utoipa::path(post, operation_id = "post_api_v1_agents_by_id_access_requests_by_request_id_approve", path = "/api/v1/agents/{id}/access-requests/{request_id}/approve", tag = "agents",
    params(("id" = String, Path), ("request_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ApproveAgentAccessRequest,
    responses((status = 200, body = AgentAccessRequestView), (status = 400, body = super::ProblemDocument),
    (status = 401, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument),
    (status = 404, body = super::ProblemDocument), (status = 409, body = super::ProblemDocument),
    (status = 413, body = super::ProblemDocument), (status = 415, body = super::ProblemDocument),
    (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn approve(
    State(service): State<Arc<dyn AgentAccessRequests>>,
    resource: AuthenticatedResourceCaller,
    Path((id, request_id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<ApproveAgentAccessRequest>, BodyProblem>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let body = match body.and_then(|body| body.into_parts().0) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = validate_access_services(&body.services) {
        return problem(error);
    }
    let Some(credential) = resource
        .credential
        .filter(|credential| !credential.value().is_empty())
    else {
        return problem(AgentAccessError::Forbidden);
    };
    // The permanent core decision ledger owns replay. A cached HTTP receipt must
    // not substitute for the request's current durable progress.
    let agent = AgentId(id);
    let result = match body.github_connection {
        Some(choice) => {
            service
                .approve_github_connection(
                    &resource.context,
                    credential.value(),
                    &agent,
                    &request_id,
                    body.services,
                    GithubConnectionApproval {
                        sandbox_id: SandboxId(choice.sandbox_id),
                        user_service_id: choice.user_service_id,
                        scope_revision: choice.scope_revision,
                        credential_generation: choice.credential_generation,
                    },
                    &key,
                )
                .await
        }
        None => {
            service
                .approve(
                    &resource.context,
                    credential.value(),
                    &agent,
                    &request_id,
                    body.services,
                    &key,
                )
                .await
        }
    };
    response(result)
}

#[utoipa::path(post, operation_id = "post_api_v1_agents_by_id_access_requests_by_request_id_deny", path = "/api/v1/agents/{id}/access-requests/{request_id}/deny", tag = "agents",
    params(("id" = String, Path), ("request_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = DenyAgentAccessRequest,
    responses((status = 200, body = AgentAccessRequestView), (status = 400, body = super::ProblemDocument),
    (status = 401, body = super::ProblemDocument), (status = 403, body = super::ProblemDocument),
    (status = 404, body = super::ProblemDocument), (status = 409, body = super::ProblemDocument),
    (status = 413, body = super::ProblemDocument), (status = 415, body = super::ProblemDocument),
    (status = 422, body = super::ProblemDocument), (status = 503, body = super::ProblemDocument)))]
pub(crate) async fn deny(
    State(service): State<Arc<dyn AgentAccessRequests>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, request_id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<DenyAgentAccessRequest>, BodyProblem>,
) -> Response {
    let key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = body.and_then(|body| body.into_parts().0) {
        return error.into_response();
    }
    response(service.deny(&caller, &AgentId(id), &request_id, &key).await)
}
fn response(value: Result<AgentAccessRequest, AgentAccessError>) -> Response {
    value
        .map(|value| Json(AgentAccessRequestView::from(value)).into_response())
        .unwrap_or_else(problem)
}
fn problem(error: AgentAccessError) -> Response {
    let (status, code) = match error {
        AgentAccessError::AccountDeletionInProgress => {
            (StatusCode::FORBIDDEN, "account_deletion_in_progress")
        }
        AgentAccessError::Invalid => (StatusCode::BAD_REQUEST, "agent_access_invalid"),
        AgentAccessError::Forbidden => (StatusCode::FORBIDDEN, "agent_access_forbidden"),
        AgentAccessError::OwnerAuthorityRequired => (
            StatusCode::FORBIDDEN,
            "agent_access_owner_authority_required",
        ),
        AgentAccessError::ServiceConnectionRequired => (
            StatusCode::CONFLICT,
            "agent_access_service_connection_required",
        ),
        AgentAccessError::NotFound => (StatusCode::NOT_FOUND, "agent_access_not_found"),
        AgentAccessError::Conflict => (StatusCode::CONFLICT, "agent_access_conflict"),
        AgentAccessError::StaleGithubApproval => {
            (StatusCode::CONFLICT, "agent_access_stale_github_approval")
        }
        AgentAccessError::Unavailable => {
            (StatusCode::SERVICE_UNAVAILABLE, "agent_access_unavailable")
        }
    };
    super::problem_response(
        status,
        super::problem_document(status, code, &error.to_string(), code),
    )
}
