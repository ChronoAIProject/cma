use super::{error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer2_core::{
    resource_access::CredentialSource,
    sandbox::agent_io::{CommandAdmissionError, CommandLedgerError, ExternalEventContext},
    AgentId,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EventContext {
    schema_version: u16,
    channel_id: String,
    conversation_id: String,
    activity: Value,
    event_ref: String,
}

pub(super) fn parse(payload: Value) -> Result<ExternalEventContext, Box<Response>> {
    let invalid = || {
        Box::new(error::field(
            "event_context",
            "invalid_event_context",
            "Supply one complete schema-version-1 event context of at most 1 MiB.",
        ))
    };
    let wire: EventContext = serde_json::from_value(payload.clone()).map_err(|_| invalid())?;
    if wire.schema_version != 1 || wire.activity.get("schema_version") != Some(&Value::from(1)) {
        return Err(invalid());
    }
    let event_id = wire
        .activity
        .get("event_id")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?
        .to_owned();
    let context = ExternalEventContext {
        source_id: wire.channel_id,
        conversation_id: wire.conversation_id,
        event_id,
        reference: wire.event_ref,
        payload,
    };
    context.validate().map_err(|_| invalid())?;
    Ok(context)
}

#[utoipa::path(get, path = "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}", tag = "provider bindings",
    params(("binding_id" = String, Path), ("conversation_id" = String, Path), ("event_id" = String, Path)),
    responses((status = 200, body = EventContext), (status = 401), (status = 403), (status = 404), (status = 503)))]
pub(super) async fn retrieve(
    State(state): State<ResponsesV2State>,
    auth: Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>,
    Path((binding, conversation, event)): Path<(String, String, String)>,
    setups: Option<Extension<std::sync::Arc<layer2_core::bot_setups::BotSetupService>>>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    if auth.context.credential_source != CredentialSource::Binding {
        return error::error(
            StatusCode::FORBIDDEN,
            "binding_key_required",
            "Event context requires a current binding key.",
            None,
        );
    }
    if let (Some(Extension(setups)), Some(credential)) = (setups, auth.credential.as_ref()) {
        match setups
            .context(&binding, credential.value(), &conversation, &event)
            .await
        {
            Ok(Some(context)) => {
                return ([(header::CACHE_CONTROL, "no-store")], Json(context)).into_response()
            }
            Ok(None) => {}
            Err(layer2_core::bots::BotError::Forbidden) => {
                return error::error(
                    StatusCode::FORBIDDEN,
                    "binding_key_required",
                    "Event context requires a current binding key.",
                    None,
                )
            }
            Err(_) => {
                return error::error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "event_context_unavailable",
                    "Event context is temporarily unavailable.",
                    None,
                )
            }
        }
    }
    let agent = match state
        .resolve_conversation(&auth, &AgentId(conversation.clone()))
        .await
    {
        Ok(agent) => agent,
        Err(error) => return *error,
    };
    let result = state
        .commands
        .external_event_context(
            &auth.context,
            auth.credential.as_ref().map(|value| value.value()),
            &agent,
            &binding,
            &conversation,
            &event,
        )
        .await;
    let response = match result {
        Ok(context) => Json(context.payload).into_response(),
        Err(CommandAdmissionError::Ledger(CommandLedgerError::NotFound)) => error::error(
            StatusCode::NOT_FOUND,
            "event_context_not_found",
            "This event context is unavailable to the binding.",
            None,
        ),
        Err(error) => super::error::admission(error),
    };
    ([(header::CACHE_CONTROL, "no-store")], response).into_response()
}
