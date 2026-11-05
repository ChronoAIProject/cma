use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;

pub(super) async fn route(
    State(relay): State<Arc<Relay>>,
    request: Request,
    next: Next,
) -> Response {
    let private = request.uri().path().starts_with(PREFIX);
    let proof = request.headers().get(PROOF).cloned();
    let Some(id) = agent_path(request.uri().path(), private) else {
        return if private || proof.is_some() {
            StatusCode::FORBIDDEN.into_response()
        } else {
            next.run(request).await
        };
    };
    // Durable admissions and reads need no local transport. Only paths that can
    // invoke a synchronous Agent runtime effect take this one-hop route.
    if !private && proof.is_none() && !needs_owner(request.uri().path(), request.method()) {
        return next.run(request).await;
    }
    let Ok(slot) = relay.slots.clone().try_acquire_owned() else {
        return unavailable();
    };
    if !relay.coordinator.is_ready() {
        return unavailable();
    }
    let body_limit = body_limit(request.uri().path());
    let Ok(_body_budget) = relay
        .body_budget
        .clone()
        .try_acquire_many_owned(body_limit as u32)
    else {
        return unavailable();
    };
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, body_limit)).await
    {
        Ok(Ok(bytes)) => bytes,
        _ => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut request = Request::from_parts(parts, Body::from(bytes.clone()));
    let destination = if let Some(raw) = proof {
        if request.headers().get_all(PROOF).iter().count() != 1 {
            return StatusCode::FORBIDDEN.into_response();
        }
        let verified = raw.to_str().ok().and_then(|raw| {
            relay
                .authentication
                .verify(
                    raw,
                    request.method(),
                    request.uri(),
                    request.headers(),
                    &bytes,
                )
                .ok()
        });
        let Some(verified) = verified else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if verified.owner.agent_id != id
            || verified.owner.instance_id != relay.coordinator.instance_id
        {
            return unavailable();
        }
        let membership = match bounded(relay.coordinator.store.membership()).await {
            Ok(membership) => membership,
            Err(_) => return unavailable(),
        };
        if membership.require_compatible().is_err()
            || !membership.members.iter().any(|member| {
                member.instance_id == verified.sender
                    && member.is_live(membership.now_ms)
                    && !member.draining
            })
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let destination = match if private
            && request
                .uri()
                .path()
                .starts_with("/internal/agent-manager/v1/control/")
        {
            relay.control_destination(&id).await
        } else {
            relay.destination(&id).await
        } {
            Ok(value) => value,
            Err(_) => return unavailable(),
        };
        if destination.token != verified.owner
            && !(verified.owner.epoch == 0
                && private
                && request
                    .uri()
                    .path()
                    .starts_with("/internal/agent-manager/v1/control/")
                && destination.token.instance_id == verified.owner.instance_id)
        {
            return unavailable();
        }
        request.headers_mut().remove(PROOF);
        request.extensions_mut().insert(VerifiedPeer);
        destination
    } else {
        if private {
            return StatusCode::FORBIDDEN.into_response();
        }
        match relay.destination(&id).await {
            Ok(value) => value,
            Err(_) => return unavailable(),
        }
    };
    if destination.member.instance_id == relay.coordinator.instance_id {
        return destination
            .authority
            .run(next.run(request))
            .await
            .unwrap_or_else(|_| unavailable());
    }
    if private {
        return unavailable();
    }
    forward(&relay, destination, request, bytes.as_ref(), slot).await
}

#[derive(Clone)]
pub(super) struct VerifiedPeer;

fn agent_path(path: &str, private: bool) -> Option<AgentId> {
    let prefix = if private {
        if path.starts_with("/internal/agent-manager/v1/control/") {
            "/internal/agent-manager/v1/control/"
        } else {
            "/internal/agent-manager/v1/live/"
        }
    } else {
        "/api/v1/agents/"
    };
    let id = path.strip_prefix(prefix)?.split('/').next()?;
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(AgentId(id.into()))
}

fn needs_owner(path: &str, method: &axum::http::Method) -> bool {
    if method == axum::http::Method::OPTIONS {
        return false;
    }
    let Some((_, suffix)) = path
        .strip_prefix("/api/v1/agents/")
        .and_then(|path| path.split_once('/'))
    else {
        return false;
    };
    matches!(
        suffix,
        "recover-worktree"
            | "skills"
            | "models"
            | "environment"
            | "attachments"
            | "files"
            | "browser"
    ) || suffix.starts_with("files/")
        || suffix == "browser/control"
}

fn body_limit(path: &str) -> usize {
    if path.ends_with("/attachments") {
        layer2_core::sandbox::agent_attachments::MAX_ATTACHMENT_BYTES
    } else if path.ends_with("/files") {
        (layer3_adapter::MAX_AGENT_FILE_BYTES.div_ceil(3) * 4 + 4096) as usize
    } else {
        crate::REQUEST_BODY_LIMIT_BYTES
    }
}

async fn forward(
    relay: &Relay,
    destination: Destination,
    request: Request,
    body: &[u8],
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut headers = HeaderMap::new();
    for (name, value) in request.headers() {
        if authentication::forwarded(name.as_str()) {
            headers.append(name, value.clone());
        }
    }
    let proof = match relay.authentication.sign(
        &relay.coordinator.instance_id,
        &destination.token,
        request.method(),
        request.uri(),
        &headers,
        body,
    ) {
        Ok(proof) => proof,
        Err(_) => return unavailable(),
    };
    let url = format!(
        "{}{}",
        destination.member.relay_address.trim_end_matches('/'),
        request
            .uri()
            .path_and_query()
            .map_or("/", |value| value.as_str())
    );
    let response = match tokio::time::timeout_at(
        deadline,
        relay
            .client
            .request(request.method().clone(), url)
            .headers(headers)
            .header(PROOF, proof)
            .body(body.to_vec())
            .send(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => return unavailable(),
    };
    let status = response.status();
    let headers = response.headers().clone();
    // Hold capacity through the response body, including slow readers. Headers
    // alone do not finish a relay, and a trickle cannot extend its total deadline.
    let stream = futures_util::stream::unfold(
        Some((response.bytes_stream(), permit)),
        move |state| async move {
            let (mut stream, permit) = state?;
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            let result = match next {
                Ok(Some(result)) => result.map_err(|_| std::io::Error::other("owner relay ended")),
                Ok(None) => return None,
                Err(_) => {
                    return Some((
                        Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "owner relay deadline exceeded",
                        )),
                        None,
                    ))
                }
            };
            Some((result, Some((stream, permit))))
        },
    );
    let mut result = Response::new(Body::from_stream(stream));
    *result.status_mut() = status;
    for (name, value) in &headers {
        if !matches!(
            name.as_str(),
            "connection"
                | "transfer-encoding"
                | "keep-alive"
                | "upgrade"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "trailer"
        ) {
            result.headers_mut().append(name, value.clone());
        }
    }
    result
}

pub(super) fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [("retry-after", "1"), ("content-type", "application/problem+json")], axum::Json(serde_json::json!({
        "type":"about:blank", "title":"Agent owner unavailable", "status":503, "code":"agent_owner_unavailable"
    }))).into_response()
}
