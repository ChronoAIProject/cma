use super::{error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{
        agent_io::{CommandStatusView, CommandSubmissionSource},
        AgentEventStreamAuthorization,
    },
    AgentId,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;

#[path = "routes_output.rs"]
pub(super) mod output;
#[path = "routes_retained_output.rs"]
pub(super) mod retained_output;

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(super) struct CreateV2ResponseRequest {
    conversation: Option<Value>,
    input: Value,
    stream: bool,
    model: Option<String>,
    metadata: Option<BTreeMap<String, String>>,
    event_context: Option<super::event_context::EventContext>,
    answer_context: Option<crate::conversation::AnswerContextRequest>,
    task_acceptance: Option<crate::rest::task_acceptance_shapes::TaskAcceptanceRequestView>,
    /// Exact provider binding generation; valid only with a binding credential.
    #[schema(minimum = 1)]
    binding_generation: Option<u64>,
    store: Option<bool>,
}
#[utoipa::path(post, path = "/api/v2/responses", tag = "responses v2", request_body = CreateV2ResponseRequest, params(("Idempotency-Key" = Option<String>, Header)), responses((status = 200, body = String, content_type = "text/event-stream"), (status = 422, body = Value)))]
pub(super) async fn create(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    submit(state, auth, headers, body, None).await
}
#[utoipa::path(post, path = "/api/v2/agents/{agent_id}/responses", tag = "responses v2", request_body = CreateV2ResponseRequest, params(("agent_id" = String, Path), ("Idempotency-Key" = Option<String>, Header)), responses((status = 200, body = String, content_type = "text/event-stream"), (status = 422, body = Value)))]
pub(super) async fn create_alias(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(agent_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    submit(state, auth, headers, body, Some(AgentId(agent_id))).await
}
async fn submit(
    state: ResponsesV2State,
    auth: Auth,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
    alias: Option<AgentId>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Some(credential) = auth.credential.as_ref() else {
        return error::unauthenticated();
    };
    let Json(mut body) = match body {
        Ok(body) => body,
        Err(_) => {
            return error::error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "A JSON response request is required.",
                Some("body"),
            )
        }
    };
    let key = if headers.get_all("idempotency-key").iter().count() > 1 {
        return error::field(
            "Idempotency-Key",
            "invalid_idempotency_key",
            "Supply one idempotency key.",
        );
    } else {
        match headers.get("idempotency-key") {
            None => AgentCommandId::generate().to_string(),
            Some(value) => match value.to_str() {
                Ok(value) => value.to_owned(),
                Err(_) => {
                    return error::field(
                        "Idempotency-Key",
                        "invalid_idempotency_key",
                        "The idempotency key is invalid.",
                    )
                }
            },
        }
    };
    let binding =
        auth.context.credential_source == layer2_core::resource_access::CredentialSource::Binding;
    if !binding && body.get("event_context").is_some() {
        return error::error(
            StatusCode::FORBIDDEN,
            "binding_key_required",
            "Event context requires a current binding key.",
            Some("event_context"),
        );
    }
    let expected_generation = match body
        .as_object_mut()
        .and_then(|body| body.remove("binding_generation"))
    {
        None => None,
        Some(value) => match value.as_u64().filter(|value| *value > 0) {
            Some(value) if binding => Some(value),
            _ => {
                return error::field(
                    "binding_generation",
                    "invalid_binding_generation",
                    "Supply a positive generation only with a binding key.",
                )
            }
        },
    };
    let binding_policy = if binding {
        match state.submission_policy(&auth, expected_generation).await {
            Ok(policy) => Some(policy),
            Err(error) => return *error,
        }
    } else {
        None
    };
    let binding_generation = binding_policy.as_ref().map(|policy| policy.generation);
    // A binding's URL and body may use different aliases of the same Agent.
    let parse_alias = if binding && body.get("conversation").is_some() {
        None
    } else {
        alias.clone()
    };
    let mut submission = match super::parse::parse(body, parse_alias, key) {
        Ok(value) => value,
        Err(error) => return *error,
    };
    let routed = if binding {
        if let (Some(router), Some(event)) =
            (&state.event_router, submission.command.external_event_mut())
        {
            match router
                .route_at_generation(
                    &auth,
                    event,
                    binding_generation.expect("binding generation pinned"),
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return super::routing::error(error),
            }
        } else {
            None
        }
    } else {
        None
    };
    submission.agent_id = if let Some(agent) = routed {
        agent
    } else {
        match state
            .resolve_conversation(
                &auth,
                &submission.agent_id,
                binding_generation,
                submission.command.external_event_mut(),
            )
            .await
        {
            Ok(id) => id,
            Err(error) => return *error,
        }
    };
    if binding {
        if let Some(alias) = alias {
            let alias = match state
                .resolve_conversation(&auth, &alias, binding_generation, None)
                .await
            {
                Ok(id) => id,
                Err(error) => return *error,
            };
            if alias != submission.agent_id {
                return error::field(
                    "conversation",
                    "conversation_alias_mismatch",
                    "conversation must match the Agent selected by the URL.",
                );
            }
        }
    }
    let policy_revision = binding_policy
        .as_ref()
        .filter(|_| submission.command.external_event().is_some())
        .and_then(|policy| policy.sender_context_revision);
    if policy_revision.is_some() && submission.command.external_event().is_some() {
        match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.apply_sender_context(
                &auth,
                &mut submission,
                binding_generation.expect("binding policy"),
            ),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return *error,
            Err(_) => return super::routing::error(super::BindingRouteError::SenderUnavailable),
        }
    }
    let admitted = match state
        .commands
        .admit_at_binding_revision(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission.clone(),
            policy_revision,
        )
        .await
    {
        Ok(value) => value,
        Err(layer2_core::sandbox::agent_io::CommandAdmissionError::Ledger(
            layer2_core::sandbox::agent_io::CommandLedgerError::IdempotencyConflict,
        )) if binding
            && policy_revision.is_none()
            && submission.command.external_event().is_some() =>
        {
            match state.replay_sender_context(&auth, &submission).await {
                Ok(Some(replay)) => match state
                    .commands
                    .admit_at_binding_revision(
                        &auth.context,
                        credential.value(),
                        CommandSubmissionSource::Api,
                        replay,
                        None,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(value) => return error::binding_admission(value),
                },
                _ => {
                    return error::binding_admission(
                        layer2_core::sandbox::agent_io::CommandLedgerError::IdempotencyConflict
                            .into(),
                    )
                }
            }
        }
        Err(value) if binding => return error::binding_admission(value),
        Err(value) => return error::admission(value),
    };
    let authorization = match state
        .reads
        .authorize_event_stream(
            &auth.context,
            Some(credential.value()),
            &admitted.receipt.agent_id,
        )
        .await
    {
        Ok(value) => value,
        Err(value) => return error::read(value),
    };
    super::stream::open(
        state,
        authorization,
        admitted.receipt,
        None,
        Some(admitted.turn_sequence.is_some()),
    )
    .await
}

#[utoipa::path(get, path = "/api/v2/responses/{response_id}", tag = "responses v2", params(("response_id" = String, Path)), responses((status = 200, body = super::V2ResponseObject), (status = 404, body = Value)))]
pub(super) async fn retrieve(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
    query: Result<Query<BTreeMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    match query { Ok(Query(query)) if query.is_empty() => {}, _ => return error::field("query", "responses_field_unsupported", "Retrieve the durable response without query parameters. Use the documented events extension for replay.") }
    let (authorization, status) = match authorize(&state, &auth, &AgentResponseId(id)).await {
        Ok(value) => value,
        Err(value) => return *value,
    };
    match super::stream::snapshot(&state, &authorization, &status.receipt).await {
        Ok((response, _)) => {
            if state.reads.shared_observation_enabled()
                && serde_json::to_vec(&response).map_or(true, |bytes| {
                    bytes.len() > layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES
                })
            {
                return super::stream::pagination_required();
            }
            Json(response).into_response()
        }
        Err(value) => value,
    }
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ReplayQuery {
    starting_after: Option<u64>,
}
#[utoipa::path(get, path = "/api/v2/responses/{response_id}/events", tag = "responses v2", params(("response_id" = String, Path), ("starting_after" = Option<u64>, Query), ("Last-Event-ID" = Option<String>, Header)), responses((status = 200, body = String, content_type = "text/event-stream")))]
pub(super) async fn replay(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<ReplayQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Ok(Query(query)) = query else {
        return error::field(
            "query",
            "invalid_replay_cursor",
            "Only starting_after is supported by this replay extension.",
        );
    };
    if headers.get_all("last-event-id").iter().count() > 1 {
        return error::field(
            "Last-Event-ID",
            "invalid_replay_cursor",
            "Supply one Last-Event-ID cursor.",
        );
    }
    let header = match headers.get("last-event-id") {
        None => None,
        Some(value) => match value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            Some(value) => Some(value),
            None => {
                return error::field(
                    "Last-Event-ID",
                    "invalid_replay_cursor",
                    "Last-Event-ID must be an unsigned event sequence number.",
                )
            }
        },
    };
    if header
        .zip(query.starting_after)
        .is_some_and(|(a, b)| a != b)
    {
        return error::field(
            "starting_after",
            "invalid_replay_cursor",
            "The query cursor and Last-Event-ID must agree.",
        );
    }
    let (authorization, status) = match authorize(&state, &auth, &AgentResponseId(id)).await {
        Ok(value) => value,
        Err(value) => return *value,
    };
    super::stream::open(
        state,
        authorization,
        status.receipt,
        query.starting_after.or(header),
        // Replay is already an established read path. Preserve its legacy
        // cursor error contract while allowing submit to use the unbound
        // header fast path; a bound replay still carries the known hint.
        status.turn_sequence.map(|_| true),
    )
    .await
}
async fn authorize(
    state: &ResponsesV2State,
    auth: &AuthenticatedResourceCaller,
    id: &AgentResponseId,
) -> Result<(AgentEventStreamAuthorization, CommandStatusView), Box<Response>> {
    let credential = auth.credential.as_ref().map(|value| value.value());
    let value = state
        .commands
        .response_status(&auth.context, credential, id)
        .await
        .map_err(|value| Box::new(error::admission(value)))?;
    let authorization = state
        .reads
        .authorize_event_stream(&auth.context, credential, &value.receipt.agent_id)
        .await
        .map_err(|value| Box::new(error::read(value)))?;
    Ok((authorization, value))
}

#[utoipa::path(get, path = "/api/v2", tag = "responses v2", responses((status = 200, body = Value)))]
pub(super) async fn capabilities(auth: Auth) -> Response {
    if auth.is_err() {
        return error::unauthenticated();
    }
    Json(serde_json::json!({
        "object":"cma.responses.capabilities", "version":2,
        "create":"/api/v2/responses", "agent_alias":"/api/v2/agents/{agent_id}/responses",
        "retrieve":"/api/v2/responses/{response_id}", "conversation":"/api/v2/conversations/{conversation_id}",
        "items":"/api/v2/conversations/{conversation_id}/items",
        "controls":{"path":"/api/v2/agents/{agent_id}/responses/{response_id}/controls",
            "observe":"GET","submit":"POST","commands":["cancel","steer","resolve_approval"],
            "idempotency_header":"Idempotency-Key","idempotency_required":true,
            "target":"bound_response_turn","completion":"command_delivery"},
        "retained_output":"/api/v2/responses/{response_id}/retained-output",
        "command_status":"/api/v2/commands/{command_id}",
        "queued_cancellation":{"path":"/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
            "observe":"GET","submit":"POST","idempotency_header":"Idempotency-Key","idempotency_required":true,
            "target":"unbound_response_command","completion":"source_cancelled"},
        "replay":{"path":"/api/v2/responses/{response_id}/events","cursor":"starting_after","header":"Last-Event-ID","durable":true},
        "creation":{"stream":true,"conversation":"existing_agent","previous_response_id":false,"store":true,"input":"user_text","model":"current_agent_settings","idempotency_header":"Idempotency-Key"},
        "output":{"standard_items":["message","mcp_call"],"text_deltas":"durable_message_commits","tools":"activity_only"},
        "companion":"/api/v1/agui/events?thread_id={agent_id}",
        "documentation":"https://github.com/ChronoAIProject/cma#public-interfaces"
    })).into_response()
}
