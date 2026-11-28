use super::*;
use axum::{
    body::{to_bytes, Body, HttpBody as _},
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use relay_diagnostics::RelayDiagnostic;

pub(super) use public_access::authorize_public;

pub(super) async fn route(
    State(relay): State<Arc<Relay>>,
    request: Request,
    next: Next,
) -> Response {
    let settings = settings_patch(request.uri().path(), request.method());
    let mut response = route_request(relay, request, next, settings).await;
    if settings {
        // Owner and authorization failures can occur before the public handler.
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    response
}

async fn route_request(
    relay: Arc<Relay>,
    request: Request,
    next: Next,
    settings: bool,
) -> Response {
    if relay.observation.get().is_some() && observation::eligible(&request) {
        return observation::route(&relay, request, next).await;
    }
    let private = request.uri().path().starts_with(PREFIX);
    let proof = request.headers().get(PROOF).cloned();
    let file_read = !private && file_read::eligible(request.uri().path(), request.method());
    let cancellation = queued_cancellation::agent_id(&request);
    let is_cancellation = cancellation.is_some();
    if let Some(id) = cancellation.as_ref().filter(|_| proof.is_none()) {
        return queued_cancellation::try_local(&relay, id, request, next).await;
    }
    let Some(id) = cancellation.or_else(|| agent_path(request.uri().path(), private)) else {
        return if private || proof.is_some() {
            StatusCode::FORBIDDEN.into_response()
        } else {
            next.run(request).await
        };
    };
    // With owner routing disabled, a Settings write still needs the owner's
    // authority: without it the ownership fence refuses every commit. Serve it
    // under a local owner and never forward it to a peer (#2267).
    let local_settings = settings && relay.settings_owner_routing.get().is_none();
    if local_settings && proof.is_some() {
        // An enabled ingress can reach a replica with this rollout still disabled.
        return StatusCode::FORBIDDEN.into_response();
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
    let mut diagnostic = RelayDiagnostic::from_request(&request);
    let observer = request
        .uri()
        .path()
        .starts_with("/internal/agent-manager/v1/live/");
    let class = capacity::class(request.uri().path(), request.method());
    let Ok(slot) = relay.acquire(class) else {
        return capacity::refused(observer);
    };
    // A draining receiver must authenticate the signed hop before reporting
    // handoff. Public ingress and unrelated coordinator failures still refuse.
    let write_handoff_route = relay.authentication.file_write_handoff_enabled()
        && !private
        && request.method() == axum::http::Method::POST
        && request.uri().path().ends_with("/files");
    let draining_file_peer =
        (file_read || write_handoff_route) && proof.is_some() && relay.coordinator.is_draining();
    if !relay.coordinator.is_ready() && !draining_file_peer {
        return unavailable_with(diagnostic.as_ref(), Refusal::CoordinatorNotReady);
    }
    let mut body_limit = body_limit(request.uri().path());
    if write_handoff_route {
        // Reserve the same bound that the collector enforces. A small framed
        // edit need not reserve the maximum upload size for the entire hop.
        if let Some(exact) = request.body().size_hint().exact() {
            body_limit = exact.min(body_limit as u64) as usize;
        }
    }
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
    // One public ingress can consume its own proof. A peer or handoff must independently verify.
    let browser_proof = request
        .extensions_mut()
        .remove::<crate::auth::relay::BrowserRelayProof>()
        .filter(|_| proof.is_none());
    let file_write =
        write_handoff_route && file_write::eligible(request.uri().path(), request.method(), &bytes);
    let write_target = if !private && !is_cancellation {
        match authorize_public(
            &relay,
            &id,
            &request,
            diagnostic.as_ref(),
            browser_proof.as_ref(),
            file_write && proof.is_none(),
        )
        .await
        {
            Ok(target) => target,
            Err(refusal) => return refusal,
        }
    } else {
        None
    };
    let (destination, reports_handoff) = if let Some(file_identity::VerifiedOwner(destination)) =
        request
            .extensions_mut()
            .remove::<file_identity::VerifiedOwner>()
    {
        if !relay.coordinator.is_ready()
            || !destination.authority.is_current()
            || bounded(relay.coordinator.store.check(&destination.token))
                .await
                .is_err()
        {
            return file_read::refusal(diagnostic.as_ref(), Refusal::AuthorityChanged);
        }
        request.headers_mut().remove(PROOF);
        request.extensions_mut().insert(VerifiedPeer);
        (destination, true)
    } else if let Some(raw) = proof {
        let verified = match relay
            .verify_peer(&request, &bytes, &id, diagnostic.as_mut())
            .await
        {
            Ok(verified) => verified,
            Err(refusal) => return refusal,
        };
        let reports_handoff = file_read && verified.reports_file_handoff();
        let reports_file_write = file_write && verified.reports_file_write_handoff();
        if reports_file_write && relay.coordinator.is_draining() {
            return file_write::refusal(
                &relay,
                raw.as_bytes(),
                diagnostic.as_ref(),
                Refusal::CoordinatorNotReady,
            );
        }
        if reports_handoff && relay.coordinator.is_draining() {
            return file_read::refusal(diagnostic.as_ref(), Refusal::CoordinatorNotReady);
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
            Err(
                reason @ (Refusal::OwnerMissing
                | Refusal::OwnerMemberUnavailable
                | Refusal::AuthorityChanged),
            ) if reports_file_write => {
                return file_write::refusal(&relay, raw.as_bytes(), diagnostic.as_ref(), reason);
            }
            Err(Refusal::OwnerMissing) if reports_handoff => {
                return file_read::refusal(diagnostic.as_ref(), Refusal::OwnerMissing);
            }
            Err(Refusal::OwnerMemberUnavailable) if reports_handoff => {
                return file_read::refusal(diagnostic.as_ref(), Refusal::OwnerMemberUnavailable);
            }
            Err(Refusal::CoordinatorNotReady)
                if reports_handoff && relay.coordinator.is_draining() =>
            {
                return file_read::refusal(diagnostic.as_ref(), Refusal::CoordinatorNotReady);
            }
            Err(reason) => return unavailable_with(diagnostic.as_ref(), reason),
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
            if reports_file_write {
                return file_write::refusal(
                    &relay,
                    raw.as_bytes(),
                    diagnostic.as_ref(),
                    Refusal::AuthorityChanged,
                );
            }
            return if reports_handoff {
                file_read::refusal(diagnostic.as_ref(), Refusal::AuthorityChanged)
            } else {
                unavailable_with(diagnostic.as_ref(), Refusal::AuthorityChanged)
            };
        }
        request.headers_mut().remove(PROOF);
        request.extensions_mut().insert(VerifiedPeer);
        if is_cancellation {
            request
                .extensions_mut()
                .insert(queued_cancellation::Charged);
        }
        (destination, reports_handoff)
    } else {
        if private {
            return StatusCode::FORBIDDEN.into_response();
        }
        match relay.destination(&id).await {
            Ok(value) => (value, false),
            Err(reason) if serves_without_owner(request.uri().path(), reason) => {
                // A stopped Agent holds no owner, and a draining owner cannot
                // accept new work. Runtime file operations do not depend on
                // owner authority, so serve the authorized request here.
                return retain(next.run(request).await, slot);
            }
            Err(reason) => return unavailable_with(diagnostic.as_ref(), reason),
        }
    };
    if destination.member.instance_id == relay.coordinator.instance_id {
        let response = destination
            .authority
            .run(async {
                relay_diagnostics::peer_write_entered(&request, destination.token.epoch);
                if file_write {
                    layer3_adapter::file_receiver_retry::scope_conditional_write(next.run(request))
                        .await
                } else {
                    file_read::run(next, request, file_read).await
                }
            })
            .await
            .unwrap_or_else(|_| {
                if reports_handoff {
                    file_read::refusal(diagnostic.as_ref(), Refusal::AuthorityChanged)
                } else {
                    unavailable_with(diagnostic.as_ref(), Refusal::AuthorityChanged)
                }
            });
        // One admission covers the owner response body, including live streams.
        return retain(response, slot);
    }
    if private || local_settings {
        // A retryable typed refusal: the caller may land on the owner next.
        return unavailable();
    }
    let claim = match file_identity::project(&relay, &request, &bytes, browser_proof.as_ref()).await
    {
        Ok(claim) => claim,
        Err(refusal) => return refusal,
    };
    let deadline = forward::deadline();
    let forwarded = forward::send(
        &relay,
        destination,
        &request,
        bytes.as_ref(),
        claim,
        deadline,
        diagnostic.as_mut(),
    )
    .await;
    let recovery = match &forwarded {
        Err(forward::Failure::WriteNotEntered) if write_target.is_some() => Some((
            relay_diagnostics::RecoveryTrigger::VerifiedWriteNotEntered,
            write_target,
        )),
        _ if file_read => forward::handoff(&forwarded).map(|trigger| (trigger, None)),
        _ => None,
    };
    if let Some((trigger, write_target)) = recovery {
        return handoff_recovery::Recovery {
            permit: slot,
            deadline,
            diagnostic: diagnostic.as_ref(),
            trigger,
            write_target,
        }
        .run(&relay, &id, request, next)
        .await;
    }
    forward::finish(forwarded, slot, deadline, diagnostic.as_ref())
}

#[derive(Clone)]
pub(super) struct VerifiedPeer;

pub(super) fn agent_path(path: &str, private: bool) -> Option<AgentId> {
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

/// File requests need no execution owner. Only an absent or no longer live
/// owner falls back locally; store and authority failures still refuse.
fn serves_without_owner(path: &str, reason: Refusal) -> bool {
    matches!(
        reason,
        Refusal::OwnerMissing | Refusal::OwnerMemberUnavailable
    ) && path
        .strip_prefix("/api/v1/agents/")
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(_, suffix)| suffix == "files" || suffix.starts_with("files/"))
}

fn settings_patch(path: &str, method: &axum::http::Method) -> bool {
    method == axum::http::Method::PATCH
        && path
            .strip_prefix("/api/v1/agents/")
            .and_then(|path| path.split_once('/'))
            .is_some_and(|(id, suffix)| !id.is_empty() && suffix == "settings")
}

pub(super) fn body_limit(path: &str) -> usize {
    if path.ends_with("/attachments") {
        layer2_core::sandbox::agent_attachments::MAX_ATTACHMENT_BYTES
    } else if path.ends_with("/files") {
        (layer3_adapter::MAX_AGENT_FILE_BYTES.div_ceil(3) * 4 + 4096) as usize
    } else {
        crate::REQUEST_BODY_LIMIT_BYTES
    }
}

pub(super) fn unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [("retry-after", "1"), ("content-type", "application/problem+json")], axum::Json(serde_json::json!({
        "type":"about:blank", "title":"Agent owner unavailable", "status":503, "code":"agent_owner_unavailable"
    }))).into_response()
}

pub(super) fn unavailable_with(diagnostic: Option<&RelayDiagnostic>, reason: Refusal) -> Response {
    if let Some(diagnostic) = diagnostic {
        diagnostic.record(reason);
    }
    unavailable()
}

pub(super) fn retain(response: Response, permit: capacity::Permit) -> Response {
    retain_until(response, permit, None)
}

pub(super) fn retain_until(
    response: Response,
    permit: capacity::Permit,
    deadline: Option<tokio::time::Instant>,
) -> Response {
    let (parts, body) = response.into_parts();
    let stream = futures_util::stream::unfold(
        Some((http_body_util::BodyStream::new(body), permit)),
        move |state| async move {
            let (mut stream, permit) = state?;
            let frame = match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, stream.next()).await {
                    Ok(frame) => frame?,
                    Err(_) => {
                        return Some((
                            Err(axum::Error::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "owner relay deadline exceeded",
                            ))),
                            None,
                        ))
                    }
                },
                None => stream.next().await?,
            };
            let state = frame.is_ok().then_some((stream, permit));
            Some((frame, state))
        },
    );
    Response::from_parts(parts, Body::new(http_body_util::StreamBody::new(stream)))
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
