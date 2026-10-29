//! A distinct endpoint for a separately provisioned native custom app.
use super::problems::{problem, protocol_headers};
use crate::{
    ingress::{launch::LaunchOutcome, relay::verify, IngressState},
    native_lark::{
        message, read_status,
        webhook::{self, Verified},
    },
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use cmaeg_core::{channel::Source, problem::ProblemCode};
use serde_json::json;
use std::time::{Duration, Instant};
pub fn router(state: IngressState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/native-lark/{channel_id}", post(callback))
            .with_state(state)
            .layer(from_fn(protocol_headers)),
    )
}
#[utoipa::path(post,path="/v1/native-lark/{channel_id}",operation_id="receiveNativeLarkEvent",tag="ingress",params(("channel_id"=String,Path),("x-lark-request-timestamp"=String,Header),("x-lark-request-nonce"=String,Header),("x-lark-signature"=String,Header)),request_body=serde_json::Value,responses((status=200,body=serde_json::Value),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn callback(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    // Lark requires acknowledgement within three seconds. Never ack uncommitted work.
    tokio::time::timeout(
        Duration::from_millis(2500),
        receive(state, channel_id, request),
    )
    .await
    .unwrap_or_else(|_| unavailable("native_lark_admission_timeout"))
}
async fn receive(state: IngressState, channel_id: String, request: Request) -> Response {
    if !state.native_lark_intake_enabled {
        return unavailable("native_lark_intake_disabled");
    }
    let received = Instant::now();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
        return unavailable("native_lark_ingress_capacity");
    };
    let (parts, body) = request.into_parts();
    let single = |name| verify::single(&parts.headers, name, true).ok().flatten();
    if !single("content-type").is_some_and(|s| {
        s.split(';')
            .next()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        );
    }
    let (Some(timestamp), Some(nonce), Some(signature)) = (
        single("x-lark-request-timestamp"),
        single("x-lark-request-nonce"),
        single("x-lark-signature"),
    ) else {
        return invalid();
    };
    let channel = match state.resolver.resolve_current(&channel_id).await {
        Ok(c) => c,
        Err(_) => return unavailable("native_lark_source_unavailable"),
    };
    let Some(Source::NativeLark(source)) = channel
        .definition
        .sources
        .iter()
        .find(|s| matches!(s, Source::NativeLark(_)))
    else {
        return invalid();
    };
    let Some(credentials) = channel.definition.secrets.native_lark.as_ref() else {
        return invalid();
    };
    let bytes = match to_bytes(body, state.config.max_body.min(256 * 1024)).await {
        Ok(b) => b,
        Err(_) => {
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                ProblemCode::PayloadTooLarge,
                "native_lark_body_too_large",
            )
        }
    };
    let event = match webhook::verify(
        &source.identity,
        credentials,
        timestamp,
        nonce,
        signature,
        &bytes,
        now,
    ) {
        Ok(Verified::Challenge(challenge)) => {
            return Json(json!({"challenge":challenge})).into_response()
        }
        Ok(Verified::Event(event)) => event,
        Err(_) => {
            state.activity.unauthenticated("native_lark");
            return invalid();
        }
    };
    if event["header"]["event_type"] == read_status::EVENT_TYPE {
        let status = match read_status::extract(&source.identity, &event, now) {
            Ok(status) => status,
            Err(_) => {
                return problem(
                    StatusCode::BAD_REQUEST,
                    ProblemCode::InvalidRequest,
                    "native_lark_read_status_invalid",
                )
            }
        };
        return match state
            .observe_native_lark_read(&channel, source, &status, now)
            .await
        {
            Ok(()) => Json(json!({"accepted":true})).into_response(),
            Err(()) => unavailable("native_lark_read_status_unavailable"),
        };
    }
    if event["header"]["event_type"] == "card.action.trigger" {
        let callback =
            match crate::native_lark::callback::Callback::extract(&event, &source.identity, now) {
                Ok(callback) => callback,
                Err(_) => {
                    return problem(
                        StatusCode::BAD_REQUEST,
                        ProblemCode::InvalidRequest,
                        "native_lark_control_invalid",
                    )
                }
            };
        return match state
            .launch_native_lark_control(channel.clone(), callback, permit)
            .await
        {
            LaunchOutcome::Admitted { .. } => Json(json!({})).into_response(),
            LaunchOutcome::Refused(error) => problem(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                error.code,
                error
                    .detail
                    .as_deref()
                    .unwrap_or("native_lark_control_refused"),
            ),
            _ => unavailable("native_lark_control_durable_admission_required"),
        };
    }
    let message = match message::extract(source, &event, now) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Json(json!({"accepted":false,"reason":"unsupported_event"})).into_response()
        }
        Err(_) => {
            return problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "native_lark_event_invalid",
            )
        }
    };
    let event_id = message.event_id(&source.identity).ok();
    match state
        .launch_native_lark(
            channel.clone(),
            source.clone(),
            message,
            received,
            now,
            permit,
        )
        .await
    {
        LaunchOutcome::Admitted { .. } => Json(json!({"accepted":true})).into_response(),
        LaunchOutcome::Refused(error) if error.status >= 500 => {
            unavailable("native_lark_admission_unavailable")
        }
        LaunchOutcome::Refused(error) => {
            state.activity.refused(
                &channel.definition.record_id,
                "native_lark",
                event_id.as_ref(),
                error.detail.as_deref().unwrap_or("native_lark_refused"),
            );
            Json(json!({"accepted":false,"reason":error.detail})).into_response()
        }
        _ => unavailable("native_lark_durable_admission_required"),
    }
}
fn invalid() -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        ProblemCode::NotAuthenticated,
        "native_lark_authentication_failed",
    )
}
fn unavailable(reason: &'static str) -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        ProblemCode::ProviderUnavailable,
        reason,
    )
}
