//! Authentication adapters implement these CMA-owned provider routes.
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDelegationIdentityRequest {
    /// Current gateway event capability or CMA-issued callback continuation.
    pub event_ref: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDelegationOperationRequest {
    /// Resolves the user from a verified event; never accepts a claimed user token.
    pub event_ref: String,
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub owner: Option<layer2_core::resource_access::ResourceOwner>,
    pub method: String,
    /// Canonical allowlisted CMA path, optionally including its normal query.
    pub path: String,
    #[serde(default)]
    pub body: Option<Value>,
    /// Required for every mutation. Reuse it for exact uncertain retries.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[utoipa::path(post, path = "/api/v1/agent-delegations/identity", tag = "agent delegations",
    request_body = AgentDelegationIdentityRequest,
    responses((status = 200, body = Object, description = "Linked subject or CMA sign-in URL; no user credential is returned"), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
#[allow(dead_code)]
pub(super) fn identity() {}

#[utoipa::path(post, path = "/api/v1/agent-delegations/operations", tag = "agent delegations",
    request_body = AgentDelegationOperationRequest,
    responses((status = 200, body = Object, description = "Operation result envelope {status,body}; accepted Responses calls return response_id and command_id"), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
#[allow(dead_code)]
pub(super) fn operation() {}
