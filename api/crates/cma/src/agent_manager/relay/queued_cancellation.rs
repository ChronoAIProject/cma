use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::responses::v2::QueuedCancellationNeedsOwner;

/// The source manager already charged the operation budget and records the
/// final outcome.
/// Only the authenticated relay or its same-process retry inserts this marker.
#[derive(Clone)]
pub(crate) struct Charged;

/// This ingress can handle an ownership handoff within the same public request.
#[derive(Clone)]
pub(crate) struct DeferQueuedCancellationActivity;

pub(super) fn agent_id(request: &Request) -> Option<AgentId> {
    if request.method() != Method::POST {
        return None;
    }
    let mut segments = request
        .uri()
        .path()
        .strip_prefix("/api/v2/agents/")?
        .split('/');
    let agent = segments.next()?;
    if agent.is_empty()
        || agent.len() > 256
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || segments.next() != Some("responses")
        || segments.next().is_none_or(str::is_empty)
        || segments.next() != Some("queued-cancellation")
        || segments.next().is_some()
    {
        return None;
    }
    Some(AgentId(agent.into()))
}

pub(super) async fn try_local(
    relay: &Relay,
    id: &AgentId,
    request: Request,
    next: Next,
) -> Response {
    let limit = crate::REQUEST_BODY_LIMIT_BYTES;
    let Ok(_budget) = relay
        .body_budget
        .clone()
        .try_acquire_many_owned(limit as u32)
    else {
        return next.run(request).await;
    };
    let (parts, body) = request.into_parts();
    let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(5), to_bytes(body, limit)).await
    else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let mut local = Request::from_parts(parts.clone(), Body::from(bytes.clone()));
    local
        .extensions_mut()
        .insert(DeferQueuedCancellationActivity);
    let mut response = next.clone().run(local).await;
    if response
        .extensions()
        .get::<QueuedCancellationNeedsOwner>()
        .is_none()
    {
        return response;
    }
    let activity = response
        .extensions_mut()
        .remove::<crate::activity::PendingActivity>();
    let retry = Request::from_parts(parts, Body::from(bytes.clone()));
    let response = forward_to_owner(relay, id, retry, &bytes, next).await;
    if let Some(activity) = activity {
        activity.record(response.status()).await;
    }
    response
}

async fn forward_to_owner(
    relay: &Relay,
    id: &AgentId,
    mut retry: Request,
    bytes: &[u8],
    next: Next,
) -> Response {
    // The local handler already proved resource authorization and the exact target.
    // The owner repeats both with the original credentials.
    let Ok(permit) = relay.acquire(capacity::Class::Ordinary) else {
        return capacity::refused(false);
    };
    let Ok(destination) = relay.destination(id).await else {
        return http::unavailable();
    };
    if destination.member.instance_id == relay.coordinator.instance_id {
        retry.extensions_mut().insert(Charged);
        let response = destination
            .authority
            .run(next.run(retry))
            .await
            .unwrap_or_else(|_| http::unavailable());
        return http::retain(response, permit);
    }
    super::forward::forward(relay, destination, retry, bytes, permit, None).await
}
