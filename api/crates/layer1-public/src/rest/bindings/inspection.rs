use super::*;

#[derive(Serialize, ToSchema)]
pub struct BindingConversationInspectionView {
    pub binding_id: String,
    pub generation: u64,
    pub conversation_id: String,
    pub agent_id: String,
    pub state: String,
    pub agent_status: Option<String>,
}

#[utoipa::path(get, operation_id = "inspect_binding_conversation",
    path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
    tag = "provider bindings",
    params(("binding_id" = String, Path), ("conversation_id" = String, Path)),
    responses((status = 200, body = BindingConversationInspectionView),
        (status = 400), (status = 401), (status = 403), (status = 404),
        (status = 409), (status = 503)))]
pub(super) async fn get(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, conversation)): Path<(String, String)>,
) -> Response {
    match state
        .service
        .inspect_conversation(&caller, &id, &conversation)
        .await
    {
        Ok(value) => {
            let view = BindingConversationView::from(value.conversation.clone());
            Json(BindingConversationInspectionView {
                binding_id: value.conversation.binding_id,
                generation: value.conversation.generation,
                conversation_id: view.conversation_id,
                agent_id: value.conversation.agent_id.to_string(),
                state: view.state,
                agent_status: value.agent_status.map(|status| status.name().into()),
            })
            .into_response()
        }
        Err(value) => error(value),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get), components(schemas(BindingConversationInspectionView)))]
struct InspectionApiDoc;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <InspectionApiDoc as utoipa::OpenApi>::openapi()
}
