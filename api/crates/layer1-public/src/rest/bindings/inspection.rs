use super::*;
use axum::Extension;
use layer2_core::sandbox::binding::{BindingProfileDrift, DriftState};
use layer2_core::sandbox::AgentInstructionPolicy;

#[derive(Clone)]
pub(super) struct ProfileDriftPolicy(Arc<dyn AgentInstructionPolicy>);

pub fn with_binding_profile_drift(
    router: Router,
    instruction_policy: Arc<dyn AgentInstructionPolicy>,
) -> Router {
    router.layer(Extension(ProfileDriftPolicy(instruction_policy)))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ProfileDriftState {
    Match,
    Drift,
    Unknown,
}

impl From<DriftState> for ProfileDriftState {
    fn from(value: DriftState) -> Self {
        match value {
            DriftState::Match => Self::Match,
            DriftState::Drift => Self::Drift,
            DriftState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ProfileDriftView {
    system_prompt: ProfileDriftState,
    skills: ProfileDriftState,
}

impl From<BindingProfileDrift> for ProfileDriftView {
    fn from(value: BindingProfileDrift) -> Self {
        Self {
            system_prompt: value.system_prompt.into(),
            skills: value.skills.into(),
        }
    }
}

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
    profile_drift: Option<ProfileDriftView>,
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
    drift_policy: Option<Extension<ProfileDriftPolicy>>,
) -> Response {
    match state
        .service
        .inspect_conversation_with_profile_drift(
            &caller,
            &id,
            &conversation,
            drift_policy
                .as_ref()
                .map(|Extension(policy)| policy.0.as_ref()),
        )
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
                profile_drift: value.profile_drift.map(Into::into),
            })
            .into_response()
        }
        Err(value) => error(value),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get),
    components(schemas(BindingConversationInspectionView, ProfileDriftView, ProfileDriftState))
)]
struct InspectionApiDoc;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <InspectionApiDoc as utoipa::OpenApi>::openapi()
}
