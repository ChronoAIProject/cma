//! Verified callback ingress with deliberate refusals acknowledged as 202.
use super::{
    chat_dispatch::AdmittedChat,
    problems::{problem, protocol_headers},
};
use crate::ingress::{
    admission::{admit, AdmissionFacts, Refusal},
    media_guard::Built,
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
    names,
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
    ids::{EventId, RecordId},
    problem::ProblemCode,
};
use serde::Serialize;
use std::time::Duration;
use tracing::Instrument;
use utoipa::ToSchema;

#[path = "callback_media_refusals.rs"]
mod media_refusals;

const SOURCE: &str = "nyxid_relay";

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
        .unwrap_or_else(|| IngressTiming::new(SOURCE));
    let Ok(permit) = state.inflight.clone().try_acquire_owned() else {
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
    // The request is consumed by admission so its body is released before
    // the acceptance wait; the launcher decides how long the permit is held.
    match admit_callback(&state, &channel_id, request, &timing).await {
        Ok(admitted) => super::chat_dispatch::dispatch(state, admitted, timing, permit).await,
        Err(response) => response,
    }
}

/// Verifies and normalizes one relay callback into an admitted chat turn.
///
/// Headers, the verified envelope and parsed copies are dropped when this
/// returns. Selected durable ingress retains bounded authenticated wire bytes
/// through commit; legacy ingress releases its body before acceptance waits.
async fn admit_callback(
    state: &IngressState,
    channel_id: &str,
    request: Request,
    timing: &IngressTiming,
) -> Result<AdmittedChat, Response> {
    let (parts, body) = request.into_parts();
    let media = verify::single(&parts.headers, "content-type", false);
    if !media.ok().flatten().is_some_and(|media| {
        media
            .split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return Err(problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemCode::UnsupportedMediaType,
            "application/json is required",
        ));
    }
    if parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > state.config.max_body as u64)
    {
        return Err(too_large());
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(body, state.config.max_body),
    )
    .await
    {
        Err(_) => {
            return Err(problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            ))
        }
        Ok(Err(_)) => return Err(too_large()),
        Ok(Ok(body)) => body,
    };
    let body_received_at = timing.received();
    let channel = match state
        .resolver
        .resolve_observed(channel_id, Some(timing))
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            let error = error.problem();
            return Err(problem(
                StatusCode::from_u16(error.status).unwrap(),
                error.code,
                error.detail.unwrap_or_default(),
            ));
        }
    };
    let verify_time = timing.verify();
    let verify_span = tracing::info_span!("cmaeg.verify", channel = %crate::telemetry::channel_hash(channel_id), event_id = tracing::field::Empty);
    let wire_platform = match verify::single(&parts.headers, "x-nyxid-platform", true) {
        Ok(Some(platform)) => platform,
        Err(error) => return Err(invalid(SOURCE, error)),
        _ => return Err(invalid(SOURCE, VerificationError("header_missing"))),
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
        return Err(problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "no matching relay source",
        ));
    };
    let headers = match CallbackHeaders::read(&parts.headers) {
        Ok(headers) => headers,
        Err(error) => return Err(invalid(SOURCE, error)),
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
        Err(error) => {
            state.activity.unauthenticated(SOURCE);
            return Err(invalid(SOURCE, error));
        }
    };
    // Authenticated from here on: every refusal is reported for the console.
    let record = channel.definition.record_id.clone();
    let media_event_id = (platform == cmaeg_core::channel::Platform::Telegram)
        .then(|| EventId::new(verified.payload.message_id.clone()).ok())
        .flatten();
    let refuse = |reason| {
        media_refusals::refuse(state, &record, channel_id, media_event_id.as_ref(), reason)
    };
    let reject = |error| authenticated_invalid(state, &record, SOURCE, error);
    let observation = if state.relay_telegram_member_joins_enabled {
        crate::ingress::relay::telegram_member_joins::observe(source, &verified.payload)
    } else {
        None
    }
    .unwrap_or_else(|| observe(source, &verified.payload, state.media_enabled));
    let mut facts = match observation {
        Ok(Observation::Message(facts)) => facts,
        Ok(Observation::Refused(reason)) => return Err(refuse(reason)),
        Err(error) => return Err(reject(error)),
    };
    if crate::interactions::lark::supported(platform) {
        if let Some(callback) = &facts.callback {
            let Some(store) = &state.interactions else {
                return Err(refuse(Refusal::InteractiveActionUnsupported));
            };
            let kind = match store
                .lark_context(
                    &channel,
                    platform,
                    &verified.payload.conversation.id,
                    &verified.payload.sender.platform_id,
                    &verified.payload.conversation.platform_id,
                    &callback.message_id,
                    &callback.data,
                )
                .await
            {
                Ok(kind) => kind,
                Err(crate::interactions::Error::Denied) => {
                    return Err(refuse(Refusal::InteractionExpiredOrInvalid))
                }
                Err(crate::interactions::Error::Unavailable) => {
                    return Err(problem(
                        StatusCode::SERVICE_UNAVAILABLE,
                        ProblemCode::ProviderUnavailable,
                        "interaction_store_unavailable",
                    ))
                }
            };
            // The provider omits chat_type on normal card callbacks. A retained
            // exact card supplies it; an explicit conflicting claim is invalid.
            if verified
                .payload
                .raw_platform_data
                .as_ref()
                .and_then(|raw| raw.pointer("/event/context/chat_type"))
                .is_some()
                && facts.conversation != kind
            {
                return Err(reject(VerificationError("lark_callback_invalid")));
            }
            facts.conversation = kind;
        }
    }
    let mut selected_admission = std::borrow::Cow::Borrowed(admission);
    if let Some(contract) = channel
        .definition
        .reply
        .thread_contract
        .as_ref()
        .filter(|_| facts.callback.is_none())
    {
        if let cmaeg_core::channel::Source::NyxidRelay {
            bot_id: Some(bot_id),
            platform: cmaeg_core::channel::Platform::Lark | cmaeg_core::channel::Platform::Feishu,
            ..
        } = source
        {
            facts.thread =
                match crate::ingress::relay::lark::thread_facts(&verified.payload, bot_id) {
                    Ok(thread) => thread.map(Box::new),
                    Err(error) => return Err(reject(error)),
                };
            if let Some(thread) = &facts.thread {
                facts.mentioned =
                    Some(thread.address == cmaeg_core::thread::ThreadAddress::Mention);
                if contract.follow_chat_ids.contains(&thread.chat_id) {
                    if let cmaeg_core::channel::AdmissionPolicy::Scoped { groups, .. } =
                        selected_admission.to_mut()
                    {
                        // Chat/sender ACL and an explicit group deny still win.
                        if *groups != cmaeg_core::channel::GroupAdmission::Deny {
                            *groups = cmaeg_core::channel::GroupAdmission::All;
                        }
                    }
                }
            }
        }
    }
    if let Err(reason) = admit(
        &selected_admission,
        &AdmissionFacts {
            sender_id: &verified.payload.sender.platform_id,
            chat_id: &verified.payload.conversation.platform_id,
            is_group: facts.conversation != cmaeg_core::activity::ConversationKind::Private,
            is_bot_sender: facts.actor == cmaeg_core::activity::ActorKind::Bot,
            receiving_bot_id: match source {
                cmaeg_core::channel::Source::NyxidRelay { bot_id, .. } => bot_id.as_deref(),
                _ => None,
            },
            mentioned: facts.mentioned,
            is_system: false,
            addressed: facts.addressed(admission),
        },
    ) {
        return Err(refuse(reason));
    }
    if channel.definition.partition.key == PartitionKey::Thread && facts.thread_id.is_none() {
        return Err(refuse(Refusal::ThreadEvidenceUnavailable));
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if verified.reply_expires_at <= now || verified.callback_deadline <= now {
        return Err(reject(VerificationError("token_time")));
    }
    let callback = facts.callback.clone();
    let route = verified.payload.conversation.id.clone();
    let request = match launch_request(
        channel,
        platform,
        verified,
        facts,
        &state.master_keys,
        body_received_at,
        now,
    ) {
        Ok(Built::Launch(request)) => *request,
        // A 202 refusal, because NyxID never retries a rejected callback.
        Ok(Built::Refused(reason)) => return Err(refuse(reason)),
        Err(reason) => {
            state.activity.refused(&record, SOURCE, None, reason);
            return Err(problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                reason,
            ));
        }
    };
    verify_span.record("event_id", request.activity.event_id.as_str());
    drop(verify_time);
    let routed_source = state.capture_routed(&request, &body, true);
    Ok(AdmittedChat {
        routed_source,
        request,
        callback,
        route,
        now,
    })
}

/// A refusal of an authenticated event: answered as [`refused`] and reported
/// to the activity observer with the same label.
pub(crate) fn authenticated_refusal(
    state: &IngressState,
    record: &RecordId,
    source: &'static str,
    reason: Refusal,
) -> Response {
    let response = refused(reason);
    if let Some(RefusalReason(label)) = response.extensions().get::<RefusalReason>() {
        state.activity.refused(record, source, None, label);
    }
    response
}

/// An authenticated event that failed normalization: answered as
/// [`invalid`] and reported to the activity observer.
pub(crate) fn authenticated_invalid(
    state: &IngressState,
    record: &RecordId,
    source: &'static str,
    error: VerificationError,
) -> Response {
    state.activity.refused(record, source, None, error.0);
    invalid(source, error)
}

pub(crate) fn refused(reason: Refusal) -> Response {
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
/// A rejected callback, counted by its bounded source and verification detail so
/// envelope drift is visible: NyxID never retries a rejected relay callback.
pub(crate) fn invalid(source: &'static str, error: VerificationError) -> Response {
    metrics::counter!(names::CALLBACK_INVALID, names::SOURCE => source, names::DETAIL => error.0)
        .increment(1);
    problem(
        StatusCode::UNAUTHORIZED,
        ProblemCode::CallbackInvalid,
        error.0,
    )
}
pub(crate) fn too_large() -> Response {
    problem(
        StatusCode::PAYLOAD_TOO_LARGE,
        ProblemCode::PayloadTooLarge,
        "callback body exceeds limit",
    )
}
