use super::tools::{event_ref, json, reply_key, result};
use crate::{
    auth::AgentCaller,
    delivery::{intents::IntentReceipt, side_messages::SideMessageDelivery},
    tools::ToolsState,
};
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::HeaderMap,
    response::Response,
    routing::get,
    Extension, Json, Router,
};
use cmaeg_core::problem::{Problem, ProblemCode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct SideMessageCapability {
    available: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SideMessageBody {
    text: String,
}

pub(super) fn routes(delivery: Option<SideMessageDelivery>) -> Router<ToolsState> {
    if let Some(delivery) = delivery {
        Router::new()
            .route(
                "/v1/events/{event_ref}/side-messages",
                get(capability).post(send),
            )
            .layer(Extension(delivery))
    } else {
        Router::new()
    }
}

#[utoipa::path(get, path = "/v1/events/{event_ref}/side-messages",
    operation_id = "getSideMessageCapability", tag = "tools",
    security(("agentBearer" = [])), params(("event_ref" = String, Path)),
    responses((status = 200, body = SideMessageCapability),
              (status = "default", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn capability(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
) -> Response {
    result(
        async {
            let available = state
                .side_message_available(caller, &event_ref(&reference)?)
                .await?;
            Ok(SideMessageCapability { available })
        }
        .await,
    )
}

#[utoipa::path(post, path = "/v1/events/{event_ref}/side-messages",
    operation_id = "sendSideMessage", tag = "tools",
    security(("agentBearer" = [])),
    params(("event_ref" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = SideMessageBody,
    responses((status = 200, body = IntentReceipt),
              (status = "default", body = Problem, content_type = "application/problem+json")))]
pub(crate) async fn send(
    State(state): State<ToolsState>,
    Extension(delivery): Extension<SideMessageDelivery>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    headers: HeaderMap,
    body: Result<Json<SideMessageBody>, JsonRejection>,
) -> Response {
    result(
        async {
            let body = json(body)?;
            let key = reply_key(&headers)?.ok_or_else(|| {
                Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "Idempotency-Key is required",
                )
            })?;
            state
                .send_side_message(caller, &event_ref(&reference)?, key, &delivery, body.text)
                .await
        }
        .await,
    )
}
