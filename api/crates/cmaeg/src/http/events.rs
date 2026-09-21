//! Authenticated structured/binary CloudEvents with optional live result streaming.
use super::problems::{problem, protocol_headers};
use crate::{
    auth::webhook,
    delivery::sync::SyncRegistry,
    ingress::{custom, launch::LaunchOutcome, IngressState},
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
use serde::Deserialize;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct CustomIngressState {
    pub ingress: IngressState,
    pub waiters: Arc<SyncRegistry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mode {
    mode: Option<String>,
}
pub fn router(state: CustomIngressState) -> Router {
    Router::new()
        .route("/v1/channels/{channel_id}/events", post(receive_event))
        .with_state(state)
        .layer(axum::middleware::from_fn(protocol_headers))
}
#[utoipa::path(post, path="/v1/channels/{channel_id}/events", operation_id="receiveCustomEvent", tag="ingress",
    params(("channel_id"=String, Path), ("mode"=Option<String>, Query, description="stream requires a configured Sync sink")),
    request_body(content=cmaeg_core::cloudevent::CloudEvent, content_type="application/cloudevents+json"),
    responses((status=202, description="Provider accepted or acceptance budget elapsed"), (status=200, content_type="text/event-stream", description="Live deltas, messages, and terminal outcome"),
        (status="default", body=cmaeg_core::problem::Problem, content_type="application/problem+json")))]
pub async fn receive_event(
    State(state): State<CustomIngressState>,
    Path(channel_id): Path<String>,
    request: Request,
) -> Response {
    let Ok(Query(mode)) = Query::<Mode>::try_from_uri(request.uri()) else {
        return invalid("invalid mode query");
    };
    let stream = match mode.mode.as_deref() {
        Some("stream") => true,
        None => false,
        _ => return invalid("invalid mode"),
    };
    let Ok(_permit) = state.ingress.inflight.clone().try_acquire_owned() else {
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
    let (parts, body) = request.into_parts();
    let media = webhook::single(&parts.headers, "content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let structured = match media {
        Some(value) if value.eq_ignore_ascii_case("application/cloudevents+json") => true,
        Some(value) if value.eq_ignore_ascii_case("application/json") => false,
        _ => {
            return problem(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ProblemCode::UnsupportedMediaType,
                "a CloudEvents structured or JSON binary body is required",
            )
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
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                ProblemCode::PayloadTooLarge,
                "event body exceeds 256 KiB",
            )
        }
        Err(_) => {
            return problem(
                StatusCode::REQUEST_TIMEOUT,
                ProblemCode::InvalidRequest,
                "body_timeout",
            )
        }
    };
    let received_at = Instant::now();
    let channel = match state.ingress.resolver.resolve(&channel_id).await {
        Ok(channel) => channel,
        Err(error) => return super::tools::result::<()>(Err(error.problem())),
    };
    let Some(Source::Custom { auth, .. }) = channel
        .definition
        .sources
        .iter()
        .find(|source| matches!(source, Source::Custom { .. }))
    else {
        return invalid("channel has no custom source");
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if !custom::authenticated(
        auth,
        &channel.definition.secrets,
        &parts.headers,
        &body,
        now,
    ) {
        return problem(
            StatusCode::UNAUTHORIZED,
            ProblemCode::NotAuthenticated,
            "custom event authentication failed",
        );
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
                return invalid("invalid or duplicate CloudEvents header");
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
        Err(_) => return invalid("invalid CloudEvent"),
    };
    let mut waiter = None;
    let reply = if stream {
        if !channel.definition.reply.sinks.contains(&Sink::Sync) {
            return invalid("stream requires a Sync sink");
        }
        let Some((id, pending)) = state.waiters.register() else {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::Internal,
                "sync_capacity",
            );
        };
        waiter = Some(pending);
        ReplyAddress::Sync { waiter: id }
    } else {
        ReplyAddress::None
    };
    let turn = match custom::activity::launch_request(
        channel,
        event,
        reply,
        &state.ingress.master_keys,
        received_at,
        now,
    ) {
        Ok(turn) => turn,
        Err(reason) => return invalid(reason),
    };
    let accepted = json!({"status":"accepted", "event_id":turn.activity.event_id, "conversation_id":turn.conversation_id});
    let elapsed = received_at.elapsed();
    let mut response = match state.ingress.launcher.launch(turn).await {
        LaunchOutcome::Refused(error) => {
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
    };
    response.headers_mut().insert(
        "server-timing",
        format!("ingress;dur={:.3}", elapsed.as_secs_f64() * 1000.0)
            .parse()
            .unwrap(),
    );
    response
}
fn invalid(detail: &str) -> Response {
    problem(StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest, detail)
}
