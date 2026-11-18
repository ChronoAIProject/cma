use super::*;
use axum::routing::post;
use layer2_core::{
    resource_access::ResourceOwner, sandbox::binding::ConversationResetRequest, AgentId, SubjectId,
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
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(reset),
    components(schemas(ResetConversationRequest, ConversationResetView))
)]
struct ResetApiDoc;
pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <ResetApiDoc as utoipa::OpenApi>::openapi()
}
