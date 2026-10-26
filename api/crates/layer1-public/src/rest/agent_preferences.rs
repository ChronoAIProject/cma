//! Public Agent name and system-instruction preferences.

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{Agent, AgentPreferencesUpdate},
    AgentId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    agents::{execute_command, AgentRoutesState},
    idempotency::{IdempotencyKey, RequestBody},
    instance_problem::InstanceProblem,
    BodyProblem,
};
use crate::{AuthenticatedCaller, AuthenticatedResourceCaller, DelegatedCredential};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateAgentPreferencesRequest {
    pub expected_revision: u64,
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AgentPreferencesView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub system_prompt: String,
    pub prompt_pending: bool,
    pub prompt_supported: bool,
    pub revision: u64,
}

impl TryFrom<UpdateAgentPreferencesRequest> for AgentPreferencesUpdate {
    type Error = InstanceProblem;
    fn try_from(value: UpdateAgentPreferencesRequest) -> Result<Self, Self::Error> {
        let update = match (value.title, value.system_prompt) {
            (Some(title), None) => Self::Title {
                expected_revision: value.expected_revision,
                title: title.trim().to_owned(),
            },
            (None, Some(system_prompt)) => Self::SystemPrompt {
                expected_revision: value.expected_revision,
                system_prompt,
            },
            _ => return Err(InstanceProblem::InvalidRequest),
        };
        update
            .validate()
            .map_err(|_| InstanceProblem::InvalidRequest)?;
        Ok(update)
    }
}
impl AgentPreferencesView {
    async fn from_agent(
        agent: &Agent,
        service: &layer2_core::sandbox::AgentOperationService,
    ) -> Self {
        Self {
            title: agent
                .conversation
                .preferences
                .title
                .clone()
                .or_else(|| agent.title.clone()),
            system_prompt: service.owner_system_prompt(agent),
            prompt_pending: agent.settings_pending(),
            prompt_supported: service.system_prompt_supported(agent).await,
            revision: agent.conversation.preferences.revision,
        }
    }
}

#[utoipa::path(get, operation_id = "read_agent_preferences", path = "/api/v1/agents/{id}/preferences", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = AgentPreferencesView),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<AgentPreferencesView>,
    ),
    InstanceProblem,
> {
    let id = AgentId(id);
    let (agent, authority) = if let Some(resource) = &resource {
        let read = state
            .reads
            .configuration_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &id,
            )
            .await?;
        (read.lineage.agent.clone(), Some(read))
    } else {
        (state.reads.lineage(&caller, &id).await?.agent, None)
    };
    let view = AgentPreferencesView::from_agent(&agent, &state.agent_operations).await;
    if let Some(authority) = &authority {
        state.reads.validate_configuration_read(authority).await?;
    }
    Ok(([(header::CACHE_CONTROL, "private, no-store")], Json(view)))
}

#[utoipa::path(patch, operation_id = "update_agent_preferences", path = "/api/v1/agents/{id}/preferences", tag = "agents",
    params(("id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateAgentPreferencesRequest,
    responses((status = 200, body = AgentPreferencesView),
    (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn update(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<UpdateAgentPreferencesRequest>, BodyProblem>,
) -> Response {
    let service = state.agent_operations.clone();
    let operation_caller = caller.clone();
    // Every request, including cached success replay, needs current permission.
    let read = if let Some(resource) = &resource {
        match state
            .reads
            .configuration_authorized(
                &resource.context,
                resource.credential.as_ref().map(DelegatedCredential::value),
                &AgentId(id.clone()),
            )
            .await
        {
            Ok(read) => Some(read),
            Err(error) => return InstanceProblem::from(error).into_response(),
        }
    } else {
        None
    };
    let reads = state.reads.clone();
    let mut response = execute_command(
        &state,
        &caller,
        key,
        format!("agent:preferences:{id}"),
        body,
        move |body| async move {
            let preferences = body.try_into()?;
            let agent = if let Some(resource) = resource {
                let authority = reads
                    .authorize_preferences_update(
                        &resource.context,
                        resource.credential.as_ref().map(DelegatedCredential::value),
                        &AgentId(id),
                        &preferences,
                    )
                    .await?;
                service.update_preferences_authorized(&authority).await?
            } else {
                service
                    .update_preferences(&operation_caller, &AgentId(id), preferences)
                    .await?
            };
            Ok(Json(AgentPreferencesView::from_agent(&agent, &service).await).into_response())
        },
    )
    .await;
    if let Some(read) = &read {
        if let Err(error) = state.reads.validate_configuration_read(read).await {
            return InstanceProblem::from(error).into_response();
        }
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}
