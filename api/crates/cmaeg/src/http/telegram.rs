//! Native Telegram webhook; authentication precedes all platform interpretation.
use super::{
    callbacks::{invalid, refused, too_large, CallbackResponse},
    problems::{problem, protocol_headers},
};
use crate::{
    ingress::{
        admission::{admit, AdmissionFacts, Refusal},
        relay::{verify, VerificationError},
        telegram::{self, Outcome},
        IngressState,
    },
    telemetry::timing::IngressTiming,
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::from_fn,
    response::Response,
    routing::post,
    Router,
};
use cmaeg_core::{
    channel::{PartitionKey, Source},
    problem::ProblemCode,
};
use sha2::{Digest, Sha256};
use std::time::Duration;
use subtle::ConstantTimeEq;

pub fn router(state: IngressState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/telegram/{channel_id}", post(callback))
            .with_state(state)
            .layer(from_fn(protocol_headers)),
    )
}
#[utoipa::path(post,path="/v1/telegram/{channel_id}",operation_id="receiveTelegramUpdate",tag="ingress",
    params(("channel_id"=String,Path),("x-telegram-bot-api-secret-token"=String,Header)),
    request_body=serde_json::Value,
    responses((status=202,body=CallbackResponse),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn callback(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let timing = request
        .extensions()
        .get::<IngressTiming>()
        .cloned()
        .unwrap_or_else(|| IngressTiming::new("telegram"));
    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
        return problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "ingress capacity exhausted",
        );
    };
    let (parts, body) = request.into_parts();
    if !verify::single(&parts.headers, "content-type", false)
        .ok()
        .flatten()
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        );
    }
    let secret = match verify::single(&parts.headers, "x-telegram-bot-api-secret-token", true) {
        Ok(Some(secret)) if secret.len() <= 256 => secret,
        _ => return invalid(VerificationError("telegram_webhook_secret")),
    };
    let channel = match state.resolver.resolve_current(&channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            let error = error.problem();
            return problem(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                error.code,
                error.detail.unwrap_or_default(),
            );
        }
    };
    let Some((bot_id, bot_username, admission)) =
        channel
            .definition
            .sources
            .iter()
            .find_map(|source| match source {
                Source::Telegram {
                    bot_id,
                    bot_username,
                    admission,
                } => Some((bot_id.clone(), bot_username.clone(), admission)),
                _ => None,
            })
    else {
        return invalid(VerificationError("telegram_source_missing"));
    };
    let Some(expected) = channel.definition.secrets.telegram_webhook_secret.as_ref() else {
        return invalid(VerificationError("telegram_webhook_secret"));
    };
    if !bool::from(
        Sha256::digest(secret.as_bytes()).ct_eq(&Sha256::digest(expected.as_str().as_bytes())),
    ) {
        return invalid(VerificationError("telegram_webhook_secret"));
    }
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|size| size > state.config.max_body)
    {
        return too_large();
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(body, state.config.max_body),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return too_large(),
        Err(_) => {
            return problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            )
        }
    };
    let raw = match serde_json::from_slice(&body) {
        Ok(raw) => raw,
        Err(_) => return invalid(VerificationError("telegram_update_invalid")),
    };
    let observed = match telegram::observe(raw, &bot_id, bot_username.as_deref()) {
        Ok(Outcome::Admitted(observed)) => *observed,
        Ok(Outcome::Refused(reason)) => return refused(reason),
        Err(error) => return invalid(error),
    };
    if let Err(reason) = admit(
        admission,
        &AdmissionFacts {
            sender_id: &observed.payload.sender.platform_id,
            chat_id: &observed.payload.conversation.platform_id,
            is_group: observed.facts.conversation
                != cmaeg_core::activity::ConversationKind::Private,
            is_bot_sender: false,
            is_system: false,
            addressed: observed.facts.addressed(admission),
        },
    ) {
        return refused(reason);
    }
    if channel.definition.partition.key == PartitionKey::Thread
        && observed.facts.thread_id.is_none()
    {
        return refused(Refusal::ThreadEvidenceUnavailable);
    }
    let callback = observed.facts.callback.clone();
    let route = observed.payload.conversation.id.clone();
    // Clear the platform spinner only after the callback has passed authentication and admission.
    if let (Some(callback), Some(client), Some(token)) = (
        &callback,
        &state.telegram,
        channel.definition.secrets.telegram_bot_token.as_ref(),
    ) {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            client.acknowledge_callback(token, &callback.id),
        )
        .await;
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let request = match telegram::launch(
        channel,
        &bot_id,
        observed,
        &state.master_keys,
        timing.received(),
        now,
    ) {
        Ok(request) => request,
        Err(error) => return invalid(error),
    };
    super::chat_dispatch::dispatch(state, request, callback, route, timing, now).await
}
