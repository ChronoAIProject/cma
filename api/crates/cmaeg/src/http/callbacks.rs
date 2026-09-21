//! Verified callback ingress with deliberate refusals acknowledged as 202.
use super::problems::{problem, protocol_headers};
use crate::ingress::{
    admission::{admit, AdmissionFacts, Refusal},
    launch::LaunchOutcome,
    relay::{
        activity::launch_request,
        telegram::{observe, Observation},
        verify::{self, CallbackHeaders},
        VerificationError,
    },
    IngressState,
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{from_fn, Next},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    channel::{PartitionKey, Source},
    problem::ProblemCode,
};
use serde::Serialize;
use std::time::{Duration, Instant};
use utoipa::ToSchema;

pub fn router(state: IngressState) -> Router {
    Router::new()
        .route("/v1/callbacks/{channel_id}", post(callback))
        .with_state(state)
        .layer(from_fn(protocol_headers))
        .layer(from_fn(timing))
}
#[derive(Clone, Copy)]
struct ReceivedAt(Instant);
#[derive(Clone, Copy)]
struct IngressDuration(Duration);
#[derive(Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CallbackResponse {
    Accepted {
        event_id: String,
        conversation_id: String,
    },
    Refused {
        reason: Refusal,
    },
}

#[utoipa::path(post, path="/v1/callbacks/{channel_id}", operation_id="receiveRelayCallback", tag="ingress",
    params(("channel_id"=String, Path)),
    responses((status=202, body=CallbackResponse), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn callback(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let received_at = request
        .extensions()
        .get::<ReceivedAt>()
        .map_or_else(Instant::now, |at| at.0);
    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
        let mut response = problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "ingress capacity exhausted",
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return response;
    };
    let (parts, body) = request.into_parts();
    let media = verify::single(&parts.headers, "content-type", false);
    if !media.ok().flatten().is_some_and(|media| {
        media
            .split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        );
    }
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > state.config.max_body as u64)
    {
        return too_large();
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(body, state.config.max_body),
    )
    .await
    {
        Err(_) => {
            return problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            )
        }
        Ok(Err(_)) => return too_large(),
        Ok(Ok(body)) => body,
    };
    let body_received_at = Instant::now();
    let channel = match state.resolver.resolve(&channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            let error = error.problem();
            return problem(
                StatusCode::from_u16(error.status).unwrap(),
                error.code,
                error.detail.unwrap_or_default(),
            );
        }
    };
    let platform = match verify::single(&parts.headers, "x-nyxid-platform", true) {
        Ok(Some(platform)) => platform,
        Err(error) => return invalid(error),
        _ => return invalid(VerificationError("header_missing")),
    };
    let Some((index, Source::NyxidRelay { admission, bot_username, .. })) = channel.definition.sources.iter().enumerate()
        .find(|(_, source)| matches!(source, Source::NyxidRelay { platform: candidate, .. } if candidate.as_str() == platform)) else {
        return problem(StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest, "no matching relay source");
    };
    let headers = match CallbackHeaders::read(&parts.headers) {
        Ok(headers) => headers,
        Err(error) => return invalid(error),
    };
    let verified = match verify::verify(
        &channel,
        index,
        &state.jwks,
        headers,
        &body,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
    .await
    {
        Ok(verified) => verified,
        Err(error) => return invalid(error),
    };
    let facts = match observe(&verified.payload, bot_username.as_deref()) {
        Ok(Observation::Message(facts)) => facts,
        Ok(Observation::Refused(reason)) => return refused(reason),
        Err(error) => return invalid(error),
    };
    if let Err(reason) = admit(
        admission,
        &AdmissionFacts {
            sender_id: &verified.payload.sender.platform_id,
            chat_id: &verified.payload.conversation.platform_id,
            is_group: facts.conversation != cmaeg_core::activity::ConversationKind::Private,
            is_bot_sender: false,
            is_system: false,
            addressed: facts.addressed(admission),
        },
    ) {
        return refused(reason);
    }
    if channel.definition.partition.key == PartitionKey::Thread && facts.thread_id.is_none() {
        return refused(Refusal::ThreadEvidenceUnavailable);
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if verified.reply_expires_at <= now || verified.callback_deadline <= now {
        return invalid(VerificationError("token_time"));
    }
    let request = match launch_request(
        channel,
        verified,
        facts,
        &state.master_keys,
        body_received_at,
        now,
    ) {
        Ok(request) => request,
        Err(reason) => {
            return problem(StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest, reason)
        }
    };
    let event_id = request.activity.event_id.to_string();
    let conversation_id = request.conversation_id.to_string();
    let ingress_duration = received_at.elapsed();
    let mut response = match state.launcher.launch(request).await {
        LaunchOutcome::Accepted { .. } | LaunchOutcome::Deferred => (
            StatusCode::ACCEPTED,
            Json(CallbackResponse::Accepted {
                event_id,
                conversation_id,
            }),
        )
            .into_response(),
        LaunchOutcome::Refused(error) => {
            let retry = error.code == ProblemCode::ProviderBusy;
            let mut response = problem(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                error.code,
                error.detail.unwrap_or_default(),
            );
            if retry {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
            }
            response
        }
    };
    response
        .extensions_mut()
        .insert(IngressDuration(ingress_duration));
    response
}
fn refused(reason: Refusal) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(CallbackResponse::Refused { reason }),
    )
        .into_response()
}
fn invalid(error: VerificationError) -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        ProblemCode::CallbackInvalid,
        error.0,
    )
}
fn too_large() -> Response {
    problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        ProblemCode::PayloadTooLarge,
        "callback body exceeds limit",
    )
}
async fn timing(mut request: Request, next: Next) -> Response {
    let started = Instant::now();
    request.extensions_mut().insert(ReceivedAt(started));
    let mut response = next.run(request).await;
    let duration = response
        .extensions()
        .get::<IngressDuration>()
        .map_or_else(|| started.elapsed(), |value| value.0);
    response.headers_mut().insert(
        "server-timing",
        HeaderValue::from_str(&format!(
            "ingress;dur={:.3}",
            duration.as_secs_f64() * 1000.0
        ))
        .unwrap(),
    );
    response
}
