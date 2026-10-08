//! Authenticated structured/binary CloudEvents with optional live result streaming.
mod routing;

use super::problems::{problem, protocol_headers};
use crate::telemetry::timing::IngressTiming;
use crate::{
    auth::webhook,
    delivery::sync::{SyncRegistry, Waiter},
    ingress::{
        custom,
        launch::{LaunchOutcome, LaunchRequest},
        IngressState,
    },
};
use axum::{
    body::to_bytes,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    channel::{Sink, Source},
    cloudevent::{parse_binary, parse_structured, MAX_CLOUD_EVENT_BYTES},
    problem::ProblemCode,
    seal::ReplyAddress,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
const MAX_ATTACHMENT_DETAIL_BYTES: usize = 256;
#[derive(Clone)]
pub struct CustomIngressState {
    pub ingress: IngressState,
    pub waiters: Arc<SyncRegistry>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct EventAccepted {
    pub status: AcceptedStatus,
    pub event_id: cmaeg_core::ids::EventId,
    pub conversation_id: cmaeg_core::ids::ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<super::consumer_admission::ConsumerAdmission>>,
}
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedStatus {
    Accepted,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mode {
    mode: Option<String>,
}
pub fn router(state: CustomIngressState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/channels/{channel_id}/events", post(receive_event))
            .with_state(state)
            .layer(axum::middleware::from_fn(protocol_headers)),
    )
}
#[utoipa::path(post, path="/v1/channels/{channel_id}/events", operation_id="receiveCustomEvent", tag="ingress",
    params(("channel_id"=String, Path), ("mode"=Option<String>, Query, description="stream requires a configured Sync sink"),
        ("webhook-id"=Option<String>, Header, description="Required for HMAC authentication"),
        ("webhook-timestamp"=Option<String>, Header, description="Required for HMAC authentication; Unix seconds within 300 seconds"),
        ("webhook-signature"=Option<String>, Header, description="Required for HMAC authentication; v1,base64 over id.timestamp.raw_body using the retained secret string bytes"),
        ("Authorization"=Option<String>, Header, description="Bearer key required for nyxid_key authentication"),
        ("ce-specversion"=Option<String>, Header, description="Required in binary mode; 1.0"),
        ("ce-id"=Option<String>, Header, description="Required in binary mode"),
        ("ce-source"=Option<String>, Header, description="Required in binary mode"),
        ("ce-type"=Option<String>, Header, description="Required in binary mode"),
        ("ce-subject"=Option<String>, Header), ("ce-time"=Option<String>, Header),
        ("ce-dataschema"=Option<String>, Header), ("ce-datacontenttype"=Option<String>, Header)),
    request_body(description="Structured CloudEvent or binary-mode JSON data. Source configuration selects HMAC or bearer authentication.", content(
        (cmaeg_core::cloudevent::CloudEvent="application/cloudevents+json"),
        (serde_json::Value="application/json"))),
    responses((status=202, body=EventAccepted, description="Provider accepted or acceptance budget elapsed"), (status=200, content_type="text/event-stream", description="Live deltas, messages, and terminal outcome"),
        (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn receive_event(
    State(state): State<CustomIngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let timing = request
        .extensions()
        .get::<IngressTiming>()
        .cloned()
        .unwrap_or_else(|| IngressTiming::new("custom"));
    let Ok(Query(mode)) = Query::<Mode>::try_from_uri(request.uri()) else {
        return invalid("invalid mode query");
    };
    let stream = match mode.mode.as_deref() {
        Some("stream") => true,
        None => false,
        _ => return invalid("invalid mode"),
    };
    let Ok(permit) = state.ingress.inflight.clone().try_acquire_owned() else {
        let mut response = problem(
            StatusCode::TOO_MANY_REQUESTS,
            ProblemCode::RateLimited,
            "ingress capacity exhausted",
        );
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("1"));
        return response;
    };
    // The request is consumed by preparation so its body is released before
    // the acceptance wait; the launcher decides how long the permit is held.
    let PreparedEvent {
        turn,
        waiter,
        mut accepted,
    } = match prepare_event(&state, &channel_id, stream, request, timing).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    let outcome = match routing::launch(&state.ingress, turn, permit, stream, &mut accepted).await {
        Ok(outcome) => outcome,
        Err(response) => return response,
    };
    match outcome {
        LaunchOutcome::Refused(error) => {
            let error = error.problem;
            let busy = error.code == ProblemCode::ProviderBusy;
            let mut response = super::tools::result::<()>(Err(error));
            if busy {
                response
                    .headers_mut()
                    .insert("retry-after", HeaderValue::from_static("2"));
            }
            response
        }
        _ => match waiter {
            Some(waiter) => Sse::new(waiter.stream())
                .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
                .into_response(),
            None => (StatusCode::ACCEPTED, Json(accepted)).into_response(),
        },
    }
}
/// Preserve legacy normalization order without copying the bounded event body.
enum EventInput {
    Normalized(Box<cmaeg_core::activity::Activity>),
    Legacy(
        Box<cmaeg_core::cloudevent::CloudEvent>,
        Vec<cmaeg_core::activity::Attachment>,
    ),
}
/// An authenticated event ready to launch, with the request already released.
struct PreparedEvent {
    turn: LaunchRequest,
    waiter: Option<Waiter>,
    accepted: EventAccepted,
}
/// Authenticates and normalizes one custom event into a launchable turn.
///
/// Taking `request` by value means `parts`, `body` and the parsed CloudEvent
/// are dropped when this returns, before the caller waits for acceptance.
/// Every early return is the handler's final response.
async fn prepare_event(
    state: &CustomIngressState,
    channel_id: &str,
    stream: bool,
    request: Request,
    timing: IngressTiming,
) -> Result<PreparedEvent, Response> {
    let (parts, body) = request.into_parts();
    let media = webhook::single(&parts.headers, "content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let structured = match media {
        Some(value) if value.eq_ignore_ascii_case("application/cloudevents+json") => true,
        Some(value) if value.eq_ignore_ascii_case("application/json") => false,
        _ => {
            return Err(problem(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ProblemCode::UnsupportedMediaType,
                "a CloudEvents structured or JSON binary body is required",
            ))
        }
    };
    let body = match tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(body, MAX_CLOUD_EVENT_BYTES),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return Err(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                ProblemCode::PayloadTooLarge,
                "event body exceeds 256 KiB",
            ))
        }
        Err(_) => {
            return Err(problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            ))
        }
    };
    let received_at = timing.received();
    let channel = match state
        .ingress
        .resolver
        .resolve_observed(channel_id, Some(&timing))
        .await
    {
        Ok(channel) => channel,
        Err(error) => return Err(super::tools::result::<()>(Err(error.problem()))),
    };
    let verify_time = timing.verify();
    let verify_span = tracing::info_span!("cmaeg.verify", channel = %crate::telemetry::channel_hash(channel_id), event_id = tracing::field::Empty);
    let verify_guard = verify_span.enter();
    let Some(Source::Custom {
        auth,
        attachments: config,
        ..
    }) = channel
        .definition
        .sources
        .iter()
        .find(|source| matches!(source, Source::Custom { .. }))
    else {
        return Err(invalid("channel has no custom source"));
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if !custom::authenticated(
        auth,
        &channel.definition.secrets,
        &parts.headers,
        &body,
        now,
    ) {
        return Err(problem(
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "custom event authentication failed",
        ));
    }
    let parsed = if structured {
        parse_structured(&body)
    } else {
        let mut headers = Vec::new();
        for name in parts
            .headers
            .keys()
            .filter(|name| name.as_str().starts_with("ce-"))
        {
            let Some(value) = webhook::single(&parts.headers, name.as_str()) else {
                return Err(invalid("invalid or duplicate CloudEvents header"));
            };
            headers.push((name.to_string(), value.to_owned()));
        }
        if !headers.iter().any(|(name, _)| name == "ce-datacontenttype") {
            headers.push(("ce-datacontenttype".into(), "application/json".into()));
        }
        parse_binary(&headers, &body)
    };
    let event = match parsed {
        Ok(event) => event,
        Err(_) => return Err(invalid("invalid CloudEvent")),
    };
    verify_span.record(
        "event_id",
        crate::telemetry::channel_hash(event.id.as_str()),
    );
    // Extraction only reads the verified body; no locator is fetched here.
    let attachments = match config {
        Some(config) if state.ingress.media_enabled => {
            match custom::attachments::extract(&event, config) {
                Ok(attachments) => attachments,
                Err(error) => {
                    tracing::info!("custom event attachment refused");
                    return Err(invalid(&attachment_invalid(&error.pointer)));
                }
            }
        }
        Some(_) => {
            tracing::debug!("custom attachments ignored while media is disabled");
            Vec::new()
        }
        None => Vec::new(),
    };
    // A routed event resolves before stream setup so the kill switch and target
    // refusals cannot be hidden by an absent sink or a full waiter registry.
    let input = if channel.definition.routing.is_some() {
        let attributes_signed =
            structured || !matches!(auth, cmaeg_core::channel::CustomAuth::Hmac { .. });
        EventInput::Normalized(Box::new(
            custom::activity::routed_activity(
                &channel,
                &state.ingress,
                event,
                attachments,
                attributes_signed,
            )
            .map_err(routing::build_error)?,
        ))
    } else {
        EventInput::Legacy(Box::new(event), attachments)
    };
    if stream
        && matches!(&input, EventInput::Normalized(activity) if !routing::stream_allowed(&state.ingress, &channel, activity))
    {
        return Err(invalid("fanout_requires_stream_all"));
    }
    let mut waiter = None;
    let reply = if stream {
        if !channel.definition.reply.sinks.contains(&Sink::Sync {}) {
            return Err(invalid("stream requires a Sync sink"));
        }
        let Some((id, pending)) = state.waiters.register() else {
            return Err(problem(
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::Internal,
                "sync_capacity",
            ));
        };
        waiter = Some(pending);
        ReplyAddress::Sync { waiter: id }
    } else {
        ReplyAddress::None
    };
    let built = match input {
        EventInput::Normalized(activity) => custom::activity::seal(
            channel,
            *activity,
            reply,
            &state.ingress.master_keys,
            received_at,
            now,
        ),
        EventInput::Legacy(event, attachments) => custom::activity::launch_request(
            channel,
            *event,
            attachments,
            reply,
            &state.ingress.master_keys,
            received_at,
            now,
        ),
    };
    let mut turn = match built {
        Ok(turn) => turn,
        Err(reason) => return Err(invalid(reason)),
    };
    if turn.channel.definition.routing.is_none() {
        crate::telemetry::routing::record(
            &turn.channel,
            &cmaeg_core::routing::RouteInput {
                activity: &turn.activity,
                chat_source: None,
                card_consumer: None,
                published: None,
                non_human_providers: &state.ingress.routing.non_human_providers,
                attributes_signed: true,
            },
            state.ingress.routing.multi_consumer_enabled,
            &Ok(cmaeg_core::routing::Resolution::Legacy),
        );
    }
    drop(verify_guard);
    drop(verify_time);
    turn.timing = Some(timing);
    let accepted = EventAccepted {
        status: AcceptedStatus::Accepted,
        event_id: turn.activity.event_id.clone(),
        conversation_id: turn.conversation_id.clone(),
        consumers: None,
    };
    Ok(PreparedEvent {
        turn,
        waiter,
        accepted,
    })
}
fn invalid(detail: &str) -> Response {
    problem(StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest, detail)
}
/// The pointer is sender-chosen and may be long, so the detail is bounded.
fn attachment_invalid(pointer: &str) -> String {
    let mut detail = format!("attachment_invalid {pointer}");
    let mut end = detail.len().min(MAX_ATTACHMENT_DETAIL_BYTES);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail.truncate(end);
    detail
}
