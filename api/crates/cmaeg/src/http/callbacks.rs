//! Verified callback ingress with deliberate refusals acknowledged as 202.
use super::problems::{problem, protocol_headers};
use crate::ingress::{
    admission::{admit, AdmissionFacts, Refusal},
    launch::LaunchOutcome,
    relay::{
        activity::launch_request,
        facts::{observe, Observation},
        verify::{self, CallbackHeaders},
        VerificationError,
    },
    IngressState,
};
use crate::telemetry::{
    http::{EventOutcome, RefusalReason},
    timing::IngressTiming,
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::from_fn,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    channel::{PartitionKey, Source},
    problem::ProblemCode,
};
use serde::Serialize;
use std::time::Duration;
use tracing::Instrument;
use utoipa::ToSchema;

pub fn router(state: IngressState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/callbacks/{channel_id}", post(callback))
            .with_state(state)
            .layer(from_fn(protocol_headers)),
    )
}
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
    params(("channel_id"=String, Path), ("x-nyxid-callback-token"=String, Header),
        ("x-nyxid-message-id"=String, Header), ("x-nyxid-platform"=String, Header),
        ("x-nyxid-timestamp"=String, Header), ("x-nyxid-user-token"=Option<String>, Header)),
    request_body=crate::ingress::relay::payload::CallbackPayload,
    responses((status=202, body=CallbackResponse), (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn callback(
    State(state): State<IngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let timing = request
        .extensions()
        .get::<IngressTiming>()
        .cloned()
        .unwrap_or_else(|| IngressTiming::new("nyxid_relay"));
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
    let body_received_at = timing.received();
    let channel = match state
        .resolver
        .resolve_observed(&channel_id, Some(&timing))
        .await
    {
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
    let verify_time = timing.verify();
    let verify_span = tracing::info_span!("cmaeg.verify", channel = %crate::telemetry::channel_hash(&channel_id), event_id = tracing::field::Empty);
    let wire_platform = match verify::single(&parts.headers, "x-nyxid-platform", true) {
        Ok(Some(platform)) => platform,
        Err(error) => return invalid(error),
        _ => return invalid(VerificationError("header_missing")),
    };
    let Some((index, source, admission, platform)) = channel
        .definition
        .sources
        .iter()
        .enumerate()
        .find_map(|(index, source)| match source {
            Source::NyxidRelay {
                platform: candidate,
                admission,
                ..
            } if candidate.matches_wire(wire_platform) => {
                Some((index, source, admission, *candidate))
            }
            _ => None,
        })
    else {
        return problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "no matching relay source",
        );
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
    .instrument(verify_span.clone())
    .await
    {
        Ok(verified) => verified,
        Err(error) => return invalid(error),
    };
    let facts = match observe(source, &verified.payload) {
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
    let callback = facts.callback.clone();
    let route = verified.payload.conversation.id.clone();
    let mut request = match launch_request(
        channel,
        platform,
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
    if let Some(callback) = callback {
        let Some(store) = &state.interactions else {
            return refused(Refusal::InteractiveActionUnsupported);
        };
        let interaction = match store
            .consume(
                &request.channel,
                &request.activity,
                &route,
                &callback.id,
                &callback.data,
                &callback.message_id,
            )
            .await
        {
            Ok(value) => value,
            Err(crate::interactions::Error::Denied) => {
                return refused(Refusal::InteractionExpiredOrInvalid)
            }
            Err(crate::interactions::Error::Unavailable) => {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ProblemCode::ProviderUnavailable,
                    "interaction_store_unavailable",
                )
            }
        };
        request.activity.kind = cmaeg_core::activity::ActivityKind::Interaction(interaction.0);
        request.activity.occurred_at = interaction.1;
        let Ok(mut opened) = request.event_ref.open(&state.master_keys, now) else {
            return invalid(VerificationError("event_ref_invalid"));
        };
        opened.payload.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
            &request.activity,
        ));
        let Ok(reference) = cmaeg_core::seal::EventRef::seal(&opened.payload, &state.master_keys)
        else {
            return invalid(VerificationError("event_ref_invalid"));
        };
        request.event_ref = reference;
    }
    verify_span.record("event_id", request.activity.event_id.as_str());
    drop(verify_time);
    request.timing = Some(timing);
    let event_id = request.activity.event_id.to_string();
    let conversation_id = request.conversation_id.to_string();
    match state.launcher.launch(request).await {
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
    }
}
fn refused(reason: Refusal) -> Response {
    let mut response = (
        StatusCode::ACCEPTED,
        Json(CallbackResponse::Refused { reason }),
    )
        .into_response();
    response.extensions_mut().insert(EventOutcome("refused"));
    let label = serde_json::to_value(reason)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    response.extensions_mut().insert(RefusalReason(label));
    response
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
