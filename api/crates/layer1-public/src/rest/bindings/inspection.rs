use super::*;

#[derive(Serialize, ToSchema)]
pub struct BindingConversationInspectionView {
    pub binding_id: String,
    pub generation: u64,
    pub incarnation: u64,
    pub conversation_id: String,
    pub agent_id: String,
    pub state: String,
    pub agent_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_drift: Option<BindingProfileDriftView>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingProfileDriftView {
    pub assigned_profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
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
                conversation_id: value.transport_conversation_id,
                incarnation: value.incarnation,
                agent_id: value.conversation.agent_id.to_string(),
                state: view.state,
                agent_status: value.agent_status.map(|status| status.name().into()),
                profile_drift: value.profile_drift.map(|drift| BindingProfileDriftView {
                    assigned_profile_digest: drift.assigned_profile_digest,
                    prompt: drift.prompt,
                    skills: drift.skills,
                    model: drift.model,
                }),
            })
            .into_response()
        }
        Err(value) => error(value),
    }
}

#[derive(Serialize, ToSchema)]
pub struct BindingConversationCatalogView {
    pub binding_id: String,
    pub generation: u64,
    pub revision: u64,
    pub transport_conversations: Vec<String>,
}

#[utoipa::path(get, operation_id = "list_binding_conversations",
    path = "/api/v1/cmaeg/bindings/{binding_id}/conversations",
    tag = "provider bindings", params(("binding_id" = String, Path)),
    responses((status = 200, body = BindingConversationCatalogView), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub(super) async fn catalog(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match state.service.conversation_catalog(&caller, &id).await {
        Ok(value) => Json(BindingConversationCatalogView {
            binding_id: value.binding_id,
            generation: value.generation,
            revision: value.revision,
            transport_conversations: value.transport_conversations,
        })
        .into_response(),
        Err(value) => error(value),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DesiredProfileRequest {
    pub profile: serde_json::Value,
    pub profile_digest: String,
}

#[derive(Serialize, ToSchema)]
pub struct DesiredProfileDriftView {
    pub profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
    pub service_scope: bool,
}

#[utoipa::path(post, operation_id = "inspect_binding_desired_profile", path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/profile-drift", tag = "provider bindings", params(("binding_id" = String, Path), ("conversation_id" = String, Path)), request_body = DesiredProfileRequest, responses((status = 200, body = Option<DesiredProfileDriftView>), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub(super) async fn desired_drift(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, conversation)): Path<(String, String)>,
    Json(request): Json<DesiredProfileRequest>,
) -> Response {
    match state
        .service
        .desired_profile_drift(
            &caller,
            &id,
            &conversation,
            request.profile,
            &request.profile_digest,
        )
        .await
    {
        Ok(value) => Json(value.map(|drift| DesiredProfileDriftView {
            profile_digest: drift.profile_digest,
            prompt: drift.prompt,
            skills: drift.skills,
            model: drift.model,
            service_scope: drift.service_scope,
        }))
        .into_response(),
        Err(value) => error(value),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get, catalog, desired_drift),
    components(schemas(
        BindingConversationInspectionView,
        BindingProfileDriftView,
        BindingConversationCatalogView
    ))
)]
struct InspectionApiDoc;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <InspectionApiDoc as utoipa::OpenApi>::openapi()
}
