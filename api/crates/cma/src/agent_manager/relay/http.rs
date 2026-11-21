use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use environment_diagnostics::EnvironmentRead;
use futures_util::StreamExt;

pub(super) async fn route(
    State(relay): State<Arc<Relay>>,
    request: Request,
    next: Next,
) -> Response {
    if relay.observation.get().is_some() && observation::eligible(&request) {
        return observation::route(&relay, request, next).await;
    }
    let private = request.uri().path().starts_with(PREFIX);
    let proof = request.headers().get(PROOF).cloned();
    let Some(id) = agent_path(request.uri().path(), private) else {
        return if private || proof.is_some() {
            StatusCode::FORBIDDEN.into_response()
        } else {
            next.run(request).await
        };
    };
    let settings = settings_patch(request.uri().path(), request.method());
    if settings && relay.settings_owner_routing.get().is_none() {
        // An enabled ingress can reach a replica with this rollout still disabled.
        return if proof.is_some() {
            StatusCode::FORBIDDEN.into_response()
        } else {
            next.run(request).await
        };
    }
    // Route operations whose publication or runtime effects require execution-owner authority.
    if !private
        && proof.is_none()
        && !settings
        && !needs_owner(request.uri().path(), request.method())
    {
        return next.run(request).await;
    }
    if !private
        && request
            .extensions()
            .get::<layer1_public::AuthenticatedSubject>()
            .is_none()
    {
        return layer1_public::auth::NotAuthenticated.into_response();
    }
    let mut environment = EnvironmentRead::from_request(&request);
    let observer = request
        .uri()
        .path()
        .starts_with("/internal/agent-manager/v1/live/");
    let class = capacity::class(request.uri().path(), request.method());
    let Ok(slot) = relay.acquire(class) else {
        return capacity::refused(observer);
    };
    if !relay.coordinator.is_ready() {
        return unavailable_with(environment.as_ref(), Refusal::CoordinatorNotReady);
    }
    let body_limit = body_limit(request.uri().path());
    let Ok(_body_budget) = relay
        .body_budget
        .clone()
        .try_acquire_many_owned(body_limit as u32)
    else {
        metrics::counter!("cma_relay_refusals_total", "class" => "ordinary", "reason" => "body_budget").increment(1);
        return capacity::refused(observer);
    };
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, body_limit)).await
    {
        Ok(Ok(bytes)) => bytes,
        _ => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut request = Request::from_parts(parts, Body::from(bytes.clone()));
    if !private {
        let Some(access) = relay.public_access.get() else {
            return unavailable_with(environment.as_ref(), Refusal::AuthorizationUnavailable);
        };
        let Some(context) = request
            .extensions()
            .get::<layer2_core::resource_access::CallerContext>()
        else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        let credential = request
            .extensions()
            .get::<layer1_public::DelegatedCredential>()
            .map(layer1_public::DelegatedCredential::value);
        if let Err(status) = access
            .authorize(public_access::PublicRequest {
                id: &id,
                method: request.method(),
                uri: request.uri(),
                headers: ingress::headers(&request),
                context,
                credential,
            })
            .await
        {
            return public_access::refusal(status);
        }
    }
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
                    ingress::headers(&request),
                    &bytes,
                )
                .ok()
        });
        let Some(verified) = verified else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if let Some(environment) = environment.as_mut() {
            environment.correlate_hop(raw.as_bytes());
        }
        if verified.owner.agent_id != id
            || verified.owner.instance_id != relay.coordinator.instance_id
        {
            return unavailable_with(environment.as_ref(), Refusal::AuthorityChanged);
        }
        let membership = match bounded(relay.coordinator.store.membership()).await {
            Ok(membership) => membership,
            Err(_) => {
                return unavailable_with(environment.as_ref(), Refusal::MembershipUnavailable)
            }
        };
        if membership.require_compatible().is_err()
            || !membership.members.iter().any(|member| {
                member.instance_id == verified.sender
                    && member.is_live(membership.now_ms)
                    && !member.draining
            })
        {
            if let Some(environment) = &environment {
                environment.record(Refusal::MembershipUnavailable);
            }
            return StatusCode::FORBIDDEN.into_response();
        }
        let destination = match if private
            && request
                .uri()
                .path()
                .starts_with("/internal/agent-manager/v1/control/")
        {
            relay
                .control_destination(&id)
                .await
                .map_err(|_| Refusal::AuthorityChanged)
        } else {
            relay.destination(&id).await
        } {
            Ok(value) => value,
            Err(reason) => return unavailable_with(environment.as_ref(), reason),
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
            return unavailable_with(environment.as_ref(), Refusal::AuthorityChanged);
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
            Err(reason) => return unavailable_with(environment.as_ref(), reason),
        }
    };
    if destination.member.instance_id == relay.coordinator.instance_id {
        let response = destination
            .authority
            .run(next.run(request))
            .await
            .unwrap_or_else(|_| unavailable_with(environment.as_ref(), Refusal::AuthorityChanged));
        // One admission covers the owner response body, including live streams.
        return retain(response, slot);
    }
    if private {
        return unavailable();
    }
    forward(
        &relay,
        destination,
        request,
        bytes.as_ref(),
        slot,
        environment,
    )
    .await
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
    ) || (method == axum::http::Method::PATCH && suffix == "preferences")
        || suffix.starts_with("files/")
        || suffix == "browser/control"
}

fn settings_patch(path: &str, method: &axum::http::Method) -> bool {
    method == axum::http::Method::PATCH
        && path
            .strip_prefix("/api/v1/agents/")
            .and_then(|path| path.split_once('/'))
            .is_some_and(|(_, suffix)| suffix == "settings")
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
    permit: capacity::Permit,
    mut environment: Option<EnvironmentRead>,
) -> Response {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut headers = HeaderMap::new();
    for (name, value) in ingress::headers(&request) {
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
        Err(_) => return unavailable_with(environment.as_ref(), Refusal::PeerSignature),
    };
    if let Some(environment) = environment.as_mut() {
        environment.correlate_hop(proof.as_bytes());
    }
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
        _ => return unavailable_with(environment.as_ref(), Refusal::PeerTransport),
    };
    let status = response.status();
    if let Some(environment) = environment
        .as_ref()
        .filter(|_| status.is_client_error() || status.is_server_error())
    {
        environment.record(Refusal::PeerResponse);
    }
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

fn unavailable_with(environment: Option<&EnvironmentRead>, reason: Refusal) -> Response {
    if let Some(environment) = environment {
        environment.record(reason);
    }
    unavailable()
}

pub(super) fn retain(response: Response, permit: capacity::Permit) -> Response {
    let (parts, body) = response.into_parts();
    let stream = futures_util::stream::unfold(
        Some((http_body_util::BodyStream::new(body), permit)),
        |state| async {
            let (mut stream, permit) = state?;
            let frame = stream.next().await?;
            let state = frame.is_ok().then_some((stream, permit));
            Some((frame, state))
        },
    );
    Response::from_parts(parts, Body::new(http_body_util::StreamBody::new(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_owner_route_is_exact_and_patch_only() {
        use axum::http::Method;
        for method in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ] {
            assert_eq!(
                needs_owner("/api/v1/agents/agt-example/preferences", &method),
                method == Method::PATCH
            );
        }
        for path in [
            "/api/v1/agents/agt-example/preferences/extra",
            "/api/v1/agents/agt-example/stop",
            "/api/v1/agents/agt-example",
            "/api/v1/sandboxes/sbx-example/preferences",
        ] {
            assert!(!needs_owner(path, &Method::PATCH), "{path}");
        }
    }

    #[test]
    fn settings_owner_route_is_exact_and_patch_only() {
        use axum::http::Method;
        let path = "/api/v1/agents/agt-example/settings";
        for method in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::PATCH,
        ] {
            assert_eq!(settings_patch(path, &method), method == Method::PATCH);
            assert!(
                !needs_owner(path, &method),
                "existing routes include {method} {path}"
            );
        }
        for (method, path) in [
            (Method::PATCH, "/api/v1/agents/agt-example/settings/extra"),
            (Method::PATCH, "/api/v2/agents/agt-example/settings"),
            (Method::POST, "/api/v1/agents/agt-example/stop"),
            (Method::DELETE, "/api/v1/agents/agt-example"),
            (Method::POST, "/api/v1/agents/agt-example/input"),
            (Method::POST, "/api/v1/agents/agt-example/steer"),
            (Method::POST, "/api/v1/agui/run"),
            (Method::GET, "/api/v2/agents/agt-example/queued-inputs"),
            (Method::POST, "/api/v2/agents/agt-example/responses"),
        ] {
            assert!(!settings_patch(path, &method), "{method} {path}");
            assert!(!needs_owner(path, &method), "{method} {path}");
        }
    }
}
