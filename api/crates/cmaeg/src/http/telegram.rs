//! Native Telegram webhook; authentication precedes all platform interpretation.
use super::{
    callbacks::{invalid, refused, too_large, CallbackResponse},
    chat_dispatch::AdmittedChat,
    problems::{problem, protocol_headers},
};
use crate::{
    ingress::{
        admission::{admit, AdmissionFacts, Refusal},
        media_guard::Built,
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

const SOURCE: &str = "telegram";

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
        .unwrap_or_else(|| IngressTiming::new(SOURCE));
    let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
        return problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "ingress capacity exhausted",
        );
    };
    // The request is consumed by admission so its body is released before
    // the acceptance wait; the launcher decides how long the permit is held.
    match admit_update(&state, &channel_id, request, &timing).await {
        Ok(admitted) => super::chat_dispatch::dispatch(state, admitted, timing, permit).await,
        Err(response) => response,
    }
}

/// Authenticates and normalizes one native update into an admitted chat turn.
///
/// Taking `request` by value means `parts`, `body` and the parsed update are
/// dropped when this returns, before the caller waits for acceptance. The
/// callback-query acknowledgement stays here so it still precedes dispatch.
async fn admit_update(
    state: &IngressState,
    channel_id: &str,
    request: Request,
    timing: &IngressTiming,
) -> Result<AdmittedChat, Response> {
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
        return Err(problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        ));
    }
    let secret = match verify::single(&parts.headers, "x-telegram-bot-api-secret-token", true) {
        Ok(Some(secret)) if secret.len() <= 256 => secret,
        _ => {
            return Err(invalid(
                SOURCE,
                VerificationError("telegram_webhook_secret"),
            ))
        }
    };
    let channel = match state.resolver.resolve_current(channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            let error = error.problem();
            return Err(problem(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                error.code,
                error.detail.unwrap_or_default(),
            ));
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
        return Err(invalid(
            SOURCE,
            VerificationError("telegram_source_missing"),
        ));
    };
    let Some(expected) = channel.definition.secrets.telegram_webhook_secret.as_ref() else {
        return Err(invalid(
            SOURCE,
            VerificationError("telegram_webhook_secret"),
        ));
    };
    if !bool::from(
        Sha256::digest(secret.as_bytes()).ct_eq(&Sha256::digest(expected.as_str().as_bytes())),
    ) {
        return Err(invalid(
            SOURCE,
            VerificationError("telegram_webhook_secret"),
        ));
    }
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|size| size > state.config.max_body)
    {
        return Err(too_large());
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(body, state.config.max_body),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return Err(too_large()),
        Err(_) => {
            return Err(problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            ))
        }
    };
    let raw = match serde_json::from_slice(&body) {
        Ok(raw) => raw,
        Err(_) => {
            return Err(invalid(
                SOURCE,
                VerificationError("telegram_update_invalid"),
            ))
        }
    };
    let observed =
        match telegram::observe(raw, &bot_id, bot_username.as_deref(), state.media_enabled) {
            Ok(Outcome::Admitted(observed)) => *observed,
            Ok(Outcome::Refused(reason)) => return Err(refused(reason)),
            Err(error) => return Err(invalid(SOURCE, error)),
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
        return Err(refused(reason));
    }
    if channel.definition.partition.key == PartitionKey::Thread
        && observed.facts.thread_id.is_none()
    {
        return Err(refused(Refusal::ThreadEvidenceUnavailable));
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
        Ok(Built::Launch(request)) => *request,
        // A 202 refusal, so Telegram does not redeliver the update.
        Ok(Built::Refused(reason)) => return Err(refused(reason)),
        Err(error) => return Err(invalid(SOURCE, error)),
    };
    Ok(AdmittedChat {
        request,
        callback,
        route,
        now,
    })
}
