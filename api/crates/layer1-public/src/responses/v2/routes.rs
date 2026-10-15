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
        agent_io::{CommandReceiptView, CommandSubmissionSource},
        AgentEventStreamAuthorization,
    },
    AgentId,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;
#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(super) struct CreateV2ResponseRequest {
    conversation: Option<Value>,
    input: Value,
    stream: bool,
    model: Option<String>,
    metadata: Option<BTreeMap<String, String>>,
    event_context: Option<super::event_context::EventContext>,
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
    let Json(body) = match body {
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
            match router.route(&auth, event).await {
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
            .resolve_conversation(&auth, &submission.agent_id)
            .await
        {
            Ok(id) => id,
            Err(error) => return *error,
        }
    };
    if binding {
        if let Some(alias) = alias {
            let alias = match state.resolve_conversation(&auth, &alias).await {
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
    let admitted = match state
        .commands
        .admit(
            &auth.context,
            credential.value(),
            CommandSubmissionSource::Api,
            submission,
        )
        .await
    {
        Ok(value) => value,
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
    super::stream::open(state, authorization, admitted.receipt, None).await
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
    let (authorization, receipt) = match authorize(&state, &auth, &AgentResponseId(id)).await {
        Ok(value) => value,
        Err(value) => return *value,
    };
    match super::stream::snapshot(&state, &authorization, &receipt).await {
        Ok((response, _)) => Json(response).into_response(),
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
    let (authorization, receipt) = match authorize(&state, &auth, &AgentResponseId(id)).await {
        Ok(value) => value,
        Err(value) => return *value,
    };
    super::stream::open(
        state,
        authorization,
        receipt,
        query.starting_after.or(header),
    )
    .await
}
async fn authorize(
    state: &ResponsesV2State,
    auth: &AuthenticatedResourceCaller,
    id: &AgentResponseId,
) -> Result<(AgentEventStreamAuthorization, CommandReceiptView), Box<Response>> {
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
    Ok((authorization, value.receipt))
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
        "command_status":"/api/v2/commands/{command_id}",
        "queued_cancellation":{"path":"/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation",
            "observe":"GET","submit":"POST","idempotency_header":"Idempotency-Key","idempotency_required":true,
            "target":"unbound_response_command","completion":"source_cancelled"},
        "replay":{"path":"/api/v2/responses/{response_id}/events","cursor":"starting_after","header":"Last-Event-ID","durable":true},
        "creation":{"stream":true,"conversation":"existing_agent","previous_response_id":false,"store":true,"input":"user_text","model":"current_agent_settings","idempotency_header":"Idempotency-Key"},
        "output":{"standard_items":["message","mcp_call"],"text_deltas":"durable_message_commits","tools":"activity_only"},
        "companion":"/api/v1/agui/events?thread_id={agent_id}",
        "documentation":"https://github.com/ChronoAIProject/cma/blob/main/docs/API_Responses.md"
    })).into_response()
}
