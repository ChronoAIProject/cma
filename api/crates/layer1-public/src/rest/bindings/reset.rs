use super::*;
use axum::routing::post;
use layer2_core::{
    resource_access::ResourceOwner,
    sandbox::binding::{ConversationResetRequest, ProfileTransferRequest},
    AgentId, SubjectId,
};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetConversationRequest {
    pub expected_agent_id: String,
    pub owner: String,
    pub organization: bool,
    pub idempotency_binding_id: String,
    pub request_digest: String,
}
#[derive(Serialize, ToSchema)]
pub struct ConversationResetView {
    pub binding_id: String,
    pub generation: u64,
    pub conversation_id: String,
    pub incarnation: u64,
    pub previous_agent_id: String,
    pub previous_conversation_id: String,
    pub successor_agent_id: String,
    pub state: String,
}

pub fn binding_conversation_reset_router(service: Arc<BindingService>, enabled: bool) -> Router {
    Router::new()
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets",
            post(reset),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-transfers",
            post(transfer),
        )
        .with_state(BindingRouteState {
            service,
            native_endpoint: None,
            native_advertisement_aliases: vec![],
            preparation_enabled: false,
            reset_enabled: enabled,
        })
        .layer(axum::middleware::from_fn(no_store))
}

#[utoipa::path(post, operation_id = "reset_binding_conversation",
    path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/resets",
    tag = "provider bindings", params(("binding_id" = String, Path), ("conversation_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ResetConversationRequest,
    responses((status = 200, body = ConversationResetView), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn reset(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((binding_id, conversation_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<ResetConversationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => return super::super::BodyProblem::from(e).into_response(),
    };
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return error(BindingError::InvalidRequest(
            "one Idempotency-Key is required".into(),
        ));
    }
    let owner = if body.organization {
        ResourceOwner::Organization {
            subject: SubjectId::new(body.owner),
        }
    } else {
        ResourceOwner::Person {
            subject: SubjectId::new(body.owner),
        }
    };
    let request = ConversationResetRequest {
        binding_id,
        conversation_id,
        expected_agent_id: AgentId(body.expected_agent_id),
        owner,
        manager: caller.subject.clone(),
        operation_id: key.into(),
        request_digest: body.request_digest,
        idempotency_binding_id: body.idempotency_binding_id,
    };
    match state
        .service
        .reset_conversation(&caller, request, state.reset_enabled)
        .await
    {
        Ok(receipt) => Json(ConversationResetView {
            binding_id: receipt.request.binding_id,
            generation: receipt.previous.generation,
            conversation_id: receipt.request.conversation_id,
            incarnation: receipt.head.incarnation,
            previous_agent_id: receipt.previous.agent_id.to_string(),
            previous_conversation_id: receipt.previous.conversation_id,
            successor_agent_id: receipt.head.successor.agent_id.to_string(),
            state: "reserved".into(),
        })
        .into_response(),
        Err(e) => error(e),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferConversationRequest {
    pub destination_binding_id: String,
    pub expected_agent_id: String,
    pub expected_generation: u64,
    pub expected_destination_generation: u64,
    pub expected_incarnation: u64,
    pub owner: String,
    pub organization: bool,
    pub request_digest: String,
}

#[derive(Serialize, ToSchema)]
pub struct ProfileTransferView {
    pub source_binding_id: String,
    pub source_generation: u64,
    pub source_incarnation: u64,
    pub source_agent_id: String,
    pub source_conversation_id: String,
    pub conversation_id: String,
    pub destination_binding_id: String,
    pub destination_generation: u64,
    pub destination_agent_id: String,
    pub destination_worktree_id: String,
    pub state: String,
}

#[utoipa::path(post, operation_id = "transfer_binding_profile_conversation",
    path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-transfers",
    tag = "provider bindings", params(("binding_id" = String, Path), ("conversation_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = TransferConversationRequest,
    responses((status = 200, body = ProfileTransferView), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
async fn transfer(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((binding_id, conversation_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<TransferConversationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => return super::super::BodyProblem::from(e).into_response(),
    };
    let key = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return error(BindingError::InvalidRequest(
            "one Idempotency-Key is required".into(),
        ));
    }
    let owner = if body.organization {
        ResourceOwner::Organization {
            subject: SubjectId::new(body.owner),
        }
    } else {
        ResourceOwner::Person {
            subject: SubjectId::new(body.owner),
        }
    };
    let request = ProfileTransferRequest {
        source_binding_id: binding_id,
        destination_binding_id: body.destination_binding_id,
        conversation_id,
        expected_agent_id: AgentId(body.expected_agent_id),
        expected_generation: body.expected_generation,
        expected_destination_generation: body.expected_destination_generation,
        expected_incarnation: body.expected_incarnation,
        owner,
        manager: caller.subject.clone(),
        operation_id: key.into(),
        request_digest: body.request_digest,
    };
    match state
        .service
        .transfer_profile_conversation(&caller, request, state.reset_enabled)
        .await
    {
        Ok(receipt) => Json(ProfileTransferView {
            source_binding_id: receipt.request.source_binding_id,
            source_generation: receipt.request.expected_generation,
            source_incarnation: receipt.request.expected_incarnation,
            source_agent_id: receipt.request.expected_agent_id.to_string(),
            source_conversation_id: receipt.source_conversation_id,
            conversation_id: receipt.request.conversation_id,
            destination_binding_id: receipt.request.destination_binding_id,
            destination_generation: receipt.destination_generation,
            destination_agent_id: receipt.destination_agent_id.to_string(),
            destination_worktree_id: receipt.destination_worktree_id.to_string(),
            state: "reserved".into(),
        })
        .into_response(),
        Err(e) => error(e),
    }
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(reset, transfer),
    components(schemas(
        ResetConversationRequest,
        ConversationResetView,
        TransferConversationRequest,
        ProfileTransferView
    ))
)]
struct ResetApiDoc;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <ResetApiDoc as utoipa::OpenApi>::openapi()
}
