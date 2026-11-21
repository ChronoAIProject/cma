//! A distinct endpoint for a separately provisioned native custom app.
use super::problems::{problem, protocol_headers};
use crate::telemetry::http_failure::FailureStage;
use crate::{
    channels::resolver::ResolveError,
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
    .unwrap_or_else(|_| {
        unavailable(
            "native_lark_admission_timeout",
            FailureStage::CallbackTimeout,
        )
    })
}
async fn receive(state: IngressState, channel_id: String, request: Request) -> Response {
    if !state.native_lark_intake_enabled {
        return unavailable("native_lark_intake_disabled", FailureStage::IntakeDisabled);
    }
    let received = Instant::now();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
        return unavailable(
            "native_lark_ingress_capacity",
            FailureStage::IngressCapacity,
        );
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
    let channel = match state.resolver.resolve_current(&channel_id).await {
        Ok(c) => c,
        Err(error) => return resolver_error(error, &parts.headers),
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
        // Missing, malformed or repeated headers cannot authenticate an event.
        // Encrypted URL-verification challenges do not use these headers.
        single("x-lark-request-timestamp").unwrap_or_default(),
        single("x-lark-request-nonce").unwrap_or_default(),
        single("x-lark-signature").unwrap_or_default(),
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
            Err(()) => unavailable(
                "native_lark_read_status_unavailable",
                FailureStage::NativeLarkReadStatus,
            ),
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
            LaunchOutcome::Refused(error) => {
                FailureStage::NativeLarkControlAdmission.response(problem(
                    StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                    error.code,
                    error
                        .detail
                        .as_deref()
                        .unwrap_or("native_lark_control_refused"),
                ))
            }
            _ => unavailable(
                "native_lark_control_durable_admission_required",
                FailureStage::NativeLarkControlAdmission,
            ),
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
        LaunchOutcome::Refused(error) if error.status >= 500 => unavailable(
            "native_lark_admission_unavailable",
            FailureStage::NativeLarkAdmission,
        ),
        LaunchOutcome::Refused(error) => {
            state.activity.refused(
                &channel.definition.record_id,
                "native_lark",
                event_id.as_ref(),
                error.detail.as_deref().unwrap_or("native_lark_refused"),
            );
            Json(json!({"accepted":false,"reason":error.detail})).into_response()
        }
        _ => unavailable(
            "native_lark_durable_admission_required",
            FailureStage::NativeLarkAdmission,
        ),
    }
}
fn invalid() -> Response {
    FailureStage::NativeLarkAuthentication.response(problem(
        StatusCode::UNAUTHORIZED,
        ProblemCode::NotAuthenticated,
        "native_lark_authentication_failed",
    ))
}
fn resolver_error(error: ResolveError, headers: &axum::http::HeaderMap) -> Response {
    let response = match error {
        ResolveError::Invalid
        | ResolveError::Tampered
        | ResolveError::NotFound
        | ResolveError::InvalidDefinition => {
            if required_signature_headers_invalid(headers) {
                invalid()
            } else {
                problem(
                    StatusCode::NOT_FOUND,
                    ProblemCode::ChannelNotFound,
                    "native_lark_source_not_found",
                )
            }
        }
        ResolveError::CreationPending => problem(
            StatusCode::CONFLICT,
            ProblemCode::Conflict,
            "native_lark_source_pending",
        ),
        ResolveError::RateLimited => problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "native_lark_source_rate_limited",
        ),
        ResolveError::Unavailable => unavailable(
            "native_lark_source_unavailable",
            FailureStage::NativeLarkSource,
        ),
    };
    FailureStage::NativeLarkSource.response(response)
}
fn required_signature_headers_invalid(headers: &axum::http::HeaderMap) -> bool {
    let single = |name| verify::single(headers, name, true).ok().flatten();
    let (Some(timestamp), Some(nonce), Some(signature)) = (
        single("x-lark-request-timestamp"),
        single("x-lark-request-nonce"),
        single("x-lark-signature"),
    ) else {
        return true;
    };
    timestamp.parse::<i64>().is_err()
        || !cmaeg_core::channel::native_lark::safe_id(nonce)
        || signature.len() != 64
        || !signature.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn unavailable(reason: &'static str, stage: FailureStage) -> Response {
    stage.response(problem(
        StatusCode::SERVICE_UNAVAILABLE,
        ProblemCode::ProviderUnavailable,
        reason,
    ))
}

#[cfg(test)]
#[path = "native_lark_tests.rs"]
mod tests;
