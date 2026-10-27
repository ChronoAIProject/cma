//! Native Meta challenge and signed webhook receiver, independent of relay ingress.
use super::problems::{problem, protocol_headers};
use crate::{
    ingress::{
        admission::{admit, AdmissionFacts},
        launch::LaunchOutcome,
        relay::verify,
        whatsapp::VerifiedWhatsappMessage,
        IngressState,
    },
    whatsapp::webhook::{self, Content},
};
use axum::{
    body::to_bytes,
    extract::{Path, RawQuery, Request, State},
    http::StatusCode,
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use cmaeg_core::{
    channel::{PartitionKey, Source},
    problem::ProblemCode,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

pub fn router(state: IngressState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/whatsapp/{channel_id}", get(challenge).post(callback))
            .with_state(state)
            .layer(from_fn(protocol_headers)),
    )
}
#[derive(Serialize, ToSchema)]
pub struct WhatsappWebhookResponse {
    pub accepted: usize,
    pub refused: usize,
}

#[utoipa::path(get, path="/v1/whatsapp/{channel_id}", operation_id="verifyNativeWhatsappWebhook", tag="ingress",
    params(("channel_id"=String,Path),("hub.mode"=String,Query),("hub.verify_token"=String,Query),("hub.challenge"=String,Query)),
    responses((status=200,body=String,content_type="text/plain"),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn challenge(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let Some(query) = query.filter(|query| query.len() <= 8192) else {
        return invalid("native_whatsapp_challenge_invalid");
    };
    let fields: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
    let single = |key: &str| {
        let mut found = fields.iter().filter(|(name, _)| name == key);
        let value = found.next()?.1.as_ref();
        if found.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    let (Some("subscribe"), Some(token), Some(challenge)) = (
        single("hub.mode"),
        single("hub.verify_token"),
        single("hub.challenge"),
    ) else {
        return invalid("native_whatsapp_challenge_invalid");
    };
    if token.len() > 256 || challenge.is_empty() || challenge.len() > 4096 {
        return invalid("native_whatsapp_challenge_invalid");
    }
    let channel = match state.resolver.resolve_current(&channel_id).await {
        Ok(channel) => channel,
        Err(_) => return invalid("native_whatsapp_source_unavailable"),
    };
    if !channel
        .definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Whatsapp { .. }))
    {
        return invalid("native_whatsapp_source_unavailable");
    }
    let Some(credentials) = &channel.definition.secrets.whatsapp else {
        return invalid("native_whatsapp_source_unavailable");
    };
    if !bool::from(Sha256::digest(token.as_bytes()).ct_eq(&Sha256::digest(
        credentials.verify_token.as_str().as_bytes(),
    ))) {
        return invalid("native_whatsapp_verify_token");
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        challenge.to_owned(),
    )
        .into_response()
}

#[utoipa::path(post, path="/v1/whatsapp/{channel_id}", operation_id="receiveNativeWhatsappWebhook", tag="ingress",
    params(("channel_id"=String,Path),("x-hub-signature-256"=String,Header)),request_body=serde_json::Value,
    responses((status=200,body=WhatsappWebhookResponse),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn callback(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let received = Instant::now();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let (parts, body) = request.into_parts();
    let content_type = verify::single(&parts.headers, "content-type", true)
        .ok()
        .flatten();
    if !content_type.is_some_and(|value| {
        value
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
    let signature = match verify::single(&parts.headers, "x-hub-signature-256", true) {
        Ok(Some(value)) if value.len() == 71 => value,
        _ => return invalid("native_whatsapp_signature"),
    };
    let channel = match state.resolver.resolve_current(&channel_id).await {
        Ok(channel) => channel,
        Err(_) => return invalid("native_whatsapp_source_unavailable"),
    };
    let Some(
        source @ Source::Whatsapp {
            admission,
            phone_number_id,
            ..
        },
    ) = channel
        .definition
        .sources
        .iter()
        .find(|source| matches!(source, Source::Whatsapp { .. }))
    else {
        return invalid("native_whatsapp_source_unavailable");
    };
    let Some(credentials) = &channel.definition.secrets.whatsapp else {
        return invalid("native_whatsapp_source_unavailable");
    };
    let cap = state
        .config
        .max_body
        .min(crate::ingress::routing_store::MAX_SOURCE_BYTES);
    let body = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, cap)).await {
        Ok(Ok(body)) => body,
        _ => {
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                ProblemCode::PayloadTooLarge,
                "native_whatsapp_body_unavailable",
            )
        }
    };
    if webhook::verify(credentials.app_secret.as_str(), signature, &body).is_err() {
        state.activity.unauthenticated("whatsapp");
        return invalid("native_whatsapp_signature");
    }
    let mut messages = match webhook::extract(source, &body) {
        Ok(messages) => messages,
        Err(_) => {
            return problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "native_whatsapp_envelope_invalid",
            )
        }
    };
    // Establish ordinary-message authority first when a batch contains both the
    // original and its reaction. Distinct reactions retain provider timestamps.
    messages.sort_by_key(|message| matches!(message.content, Content::Reaction { .. }));
    let mut result = WhatsappWebhookResponse {
        accepted: 0,
        refused: 0,
    };
    for message in messages {
        let event_id = webhook::event_id(source, &message.id).ok();
        if message.timestamp > now + 300
            || message.timestamp < now - crate::whatsapp::receipts::RETENTION
            || channel.definition.partition.key == PartitionKey::Thread
            || admit(
                admission,
                &AdmissionFacts {
                    receiving_bot_id: Some(phone_number_id),
                    mentioned: Some(false),
                    sender_id: &message.sender,
                    chat_id: message.conversation(),
                    is_group: message.group_id.is_some(),
                    is_bot_sender: false,
                    is_system: false,
                    addressed: Some(true),
                },
            )
            .is_err()
        {
            state.activity.refused(
                &channel.definition.record_id,
                "whatsapp",
                event_id.as_ref(),
                "native_whatsapp_admission_or_timestamp_refused",
            );
            result.refused += 1;
            continue;
        }
        let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
            return problem(
                StatusCode::TOO_MANY_REQUESTS,
                ProblemCode::RateLimited,
                "native_whatsapp_ingress_capacity",
            );
        };
        let verified = VerifiedWhatsappMessage {
            channel: channel.clone(),
            source: source.clone(),
            message,
            body: body.to_vec(),
            received,
            received_at: now,
        };
        match state.launch_whatsapp_verified(verified, permit).await {
            LaunchOutcome::Admitted { .. } => result.accepted += 1,
            LaunchOutcome::Refused(refusal) if refusal.status >= 500 => {
                let mut response = problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ProblemCode::ProviderUnavailable,
                    refusal
                        .detail
                        .unwrap_or_else(|| "native_whatsapp_admission_unavailable".into()),
                );
                response
                    .headers_mut()
                    .insert("retry-after", "5".parse().unwrap());
                return response;
            }
            LaunchOutcome::Refused(refusal) => {
                state.activity.refused(
                    &channel.definition.record_id,
                    "whatsapp",
                    event_id.as_ref(),
                    refusal
                        .detail
                        .as_deref()
                        .unwrap_or("native_whatsapp_admission_refused"),
                );
                result.refused += 1;
            }
            _ => result.refused += 1,
        }
    }
    Json(result).into_response()
}
fn invalid(reason: &'static str) -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        ProblemCode::NotAuthenticated,
        reason,
    )
}
