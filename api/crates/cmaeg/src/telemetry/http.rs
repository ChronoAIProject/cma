//! Route templates and bounded classifications prevent credentials reaching logs or labels.
use super::{http_failure::FailureStage, names, timing::IngressTiming};
use axum::{
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request},
    middleware::{from_fn, Next},
    response::Response,
    Router,
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
};
use tracing::Instrument;

#[derive(Clone)]
struct Observed;
#[derive(Clone)]
pub(crate) struct EventOutcome(pub &'static str);
pub(crate) use cmaeg_engine::RefusalReason;

pub fn instrument(router: Router) -> Router {
    router
        // Innermost, so a handler panic is answered and logged inside its
        // request span and `request finished` records the 500.
        .layer(CatchPanicLayer::custom(
            crate::http::problems::panic_response,
        ))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(from_fn(observe))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
async fn observe(mut request: Request, next: Next) -> Response {
    if request.extensions().get::<Observed>().is_some() {
        return next.run(request).await;
    }
    request.extensions_mut().insert(Observed);
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let request_hash = correlation_hash(
        request
            .headers()
            .get("x-request-id")
            .map_or(b"invalid", |value| value.as_bytes()),
    );
    // Use Axum's decoded parameter, never the URL or a user-controlled route label.
    let (mut parts, body) = request.into_parts();
    let channel_hash = RawPathParams::from_request_parts(&mut parts, &())
        .await
        .ok()
        .and_then(|params| {
            params
                .iter()
                .find(|(key, _)| *key == "channel_id")
                .map(|(_, value)| correlation_hash(value.as_bytes()))
        });
    let method = match parts.method.as_str() {
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "CONNECT" | "TRACE" => {
            parts.method.as_str()
        }
        _ => "OTHER",
    }
    .to_owned();
    let mut request = Request::from_parts(parts, body);
    let span = tracing::info_span!("cmaeg.request", request_hash, route);
    crate::providers::attach_context_request_hash(&span, &request_hash);
    let source = match route.as_str() {
        "/v1/callbacks/{channel_id}" => Some("nyxid_relay"),
        "/v1/telegram/{channel_id}" => Some("telegram"),
        "/v1/channels/{channel_id}/events" => Some("custom"),
        _ => None,
    };
    let timing = span.in_scope(|| source.map(IngressTiming::new));
    if let Some(timing) = &timing {
        request.extensions_mut().insert(timing.clone());
    }
    let tools = route == "/mcp"
        || matches!(
            route.as_str(),
            "/v1/events/{event_ref}/replies"
                | "/v1/messages/{message_ref}/update"
                | "/v1/events/{event_ref}/typing"
                | "/v1/events/{event_ref}/context"
                | "/v1/events/{event_ref}/attachments/{index}"
        );
    async move {
        let started = std::time::Instant::now();
        let mut response = super::turn_trace::ingress::observe(request, next).await;
        if let Some(timing) = timing {
            response.headers_mut().insert("server-timing", timing.header().parse().unwrap());
            let outcome = response.extensions().get::<EventOutcome>().map(|value| value.0)
                .unwrap_or(if response.status().is_success() { "accepted" } else { "rejected" });
            metrics::counter!(names::EVENTS, names::SOURCE => source.unwrap(), names::OUTCOME => outcome).increment(1);
            if let Some(reason) = response.extensions().get::<RefusalReason>() {
                metrics::counter!(names::REFUSALS, names::REASON => reason.0.clone()).increment(1);
            }
        } else if tools {
            response.headers_mut().insert("server-timing", format!("deliver;dur={:.3}", started.elapsed().as_secs_f64() * 1000.0).parse().unwrap());
        }
        let server_timing = response.headers().get("server-timing")
            .filter(|_| source.is_some() || tools)
            .and_then(|value| value.to_str().ok());
        let problem_code = response.extensions().get::<RefusalReason>().map(|code| code.0.as_str())
            .unwrap_or(if response.status().is_server_error() { "unclassified" } else { "none" });
        let failure_stage = response.extensions().get::<FailureStage>().map(|stage| stage.as_str())
            .unwrap_or(if response.status().is_client_error() || response.status().is_server_error() { "unclassified" } else { "none" });
        // These are response-header timings. Streaming body completion is measured separately.
        tracing::info!(status = response.status().as_u16(), server_timing,
            method, elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            problem_code, failure_stage, request_hash, channel_hash,
            "request finished");
        response
    }.instrument(span).await
}

pub(crate) fn correlation_hash(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
