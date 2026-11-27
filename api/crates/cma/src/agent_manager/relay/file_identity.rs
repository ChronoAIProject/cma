//! Accept file-preview authority only after verifying the exact owner hop.
use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use relay_diagnostics::RelayDiagnostic;

pub(super) const HEADER: &str = "x-cma-file-preview-identity";
const MAX_HEADER_BYTES: usize = 3072;
const PREVIEW_ROUTE: &str = "/api/v1/agents/{id}/files/content";

#[derive(Clone)]
pub(super) struct VerifiedOwner(pub Destination);

fn eligible(request: &Request) -> bool {
    request.method() == Method::GET
        && request
            .extensions()
            .get::<MatchedPath>()
            .is_some_and(|path| path.as_str() == PREVIEW_ROUTE)
}

pub(super) fn project<'a>(
    relay: &'a Relay,
    request: &Request,
    body: &[u8],
    proof: Option<&'a crate::auth::relay::BrowserRelayProof>,
) -> impl std::future::Future<Output = Result<Option<HeaderValue>, Response>> + use<'a> {
    let proof = proof.filter(|_| {
        relay.file_preview_peer_identity.get().is_some() && body.is_empty() && eligible(request)
    });
    let method = request.method().clone();
    let uri = request.uri().clone();
    async move {
        let Some(proof) = proof else {
            return Ok(None);
        };
        let policy = relay.public_access.get().ok_or_else(http::unavailable)?;
        let claim = policy
            .project_browser(proof, &method, &uri)
            .await
            .map_err(|status| status.into_response())?;
        let bytes = serde_json::to_vec(&claim).map_err(|_| http::unavailable())?;
        let value = URL_SAFE_NO_PAD.encode(bytes);
        if value.len() > MAX_HEADER_BYTES {
            return Err(http::unavailable());
        }
        let mut value = HeaderValue::from_str(&value).map_err(|_| http::unavailable())?;
        value.set_sensitive(true);
        Ok(Some(value))
    }
}

pub(super) async fn receive(
    State(relay): State<Arc<Relay>>,
    mut request: Request,
    next: Next,
) -> Response {
    if !request.headers().contains_key(PROOF) {
        request.headers_mut().remove(HEADER);
        return next.run(request).await;
    }
    if !request.headers().contains_key(HEADER) {
        return next.run(request).await;
    }
    if !eligible(&request)
        || request.headers().get_all(HEADER).iter().count() != 1
        || request.headers()[HEADER].len() > MAX_HEADER_BYTES
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(id) = http::agent_path(request.uri().path(), false) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let (parts, body) = request.into_parts();
    match tokio::time::timeout(Duration::from_secs(2), to_bytes(body, 0)).await {
        Ok(Ok(bytes)) if bytes.is_empty() => {}
        _ => return StatusCode::FORBIDDEN.into_response(),
    }
    let mut request = Request::from_parts(parts, Body::empty());
    let mut diagnostic = RelayDiagnostic::from_request(&request);
    let verified = match relay
        .verify_peer(&request, &[], &id, diagnostic.as_mut())
        .await
    {
        Ok(proof) => proof,
        Err(refusal) => return refusal,
    };
    if relay.coordinator.is_draining() {
        return file_read::refusal(diagnostic.as_ref(), Refusal::CoordinatorNotReady);
    }
    let destination = match relay.destination(&id).await {
        Ok(destination) if destination.token == verified.owner => destination,
        Ok(_) => return file_read::refusal(diagnostic.as_ref(), Refusal::AuthorityChanged),
        Err(reason @ (Refusal::OwnerMissing | Refusal::OwnerMemberUnavailable)) => {
            return file_read::refusal(diagnostic.as_ref(), reason);
        }
        Err(reason) => return http::unavailable_with(diagnostic.as_ref(), reason),
    };
    let value = &request.headers()[HEADER];
    let claim = URL_SAFE_NO_PAD
        .decode(value.as_bytes())
        .ok()
        .and_then(|value| {
            serde_json::from_slice::<crate::auth::relay::PeerBrowserClaim>(&value).ok()
        });
    let Some(claim) = claim else {
        return StatusCode::FORBIDDEN.into_response();
    };
    // The proof and nonce have already been consumed. The normal relay still
    // runs resource policy, capacity admission and the local owner fence.
    request.headers_mut().remove(HEADER);
    request.extensions_mut().insert(claim);
    request.extensions_mut().insert(VerifiedOwner(destination));
    next.run(request).await
}

impl Relay {
    pub(super) fn verify_peer<'a>(
        &'a self,
        request: &Request,
        bytes: &[u8],
        id: &AgentId,
        mut diagnostic: Option<&'a mut RelayDiagnostic>,
    ) -> impl std::future::Future<Output = Result<authentication::Proof, Response>> + use<'a> {
        let proof = (|| {
            let mut headers = request.headers().get_all(PROOF).iter();
            let raw = headers.next().filter(|_| headers.next().is_none())?;
            let verified = self
                .authentication
                .verify(
                    raw.to_str().ok()?,
                    request.method(),
                    request.uri(),
                    ingress::headers(request),
                    bytes,
                )
                .ok()?;
            if let Some(diagnostic) = diagnostic.as_mut() {
                diagnostic.correlate_hop(raw.as_bytes());
            }
            let for_receiver = &verified.owner.agent_id == id
                && verified.owner.instance_id == self.coordinator.instance_id;
            Some((verified, for_receiver))
        })();
        async move {
            let Some((verified, for_receiver)) = proof else {
                return Err(StatusCode::FORBIDDEN.into_response());
            };
            if !for_receiver {
                return Err(http::unavailable_with(
                    diagnostic.as_deref(),
                    Refusal::AuthorityChanged,
                ));
            }
            let membership = match bounded(self.coordinator.store.membership()).await {
                Ok(membership) => membership,
                Err(_) => {
                    return Err(http::unavailable_with(
                        diagnostic.as_deref(),
                        Refusal::MembershipUnavailable,
                    ))
                }
            };
            if membership.require_compatible().is_err()
                || !membership.members.iter().any(|member| {
                    member.instance_id == verified.sender
                        && member.is_live(membership.now_ms)
                        && !member.draining
                })
            {
                if let Some(diagnostic) = diagnostic {
                    diagnostic.record(Refusal::MembershipUnavailable);
                }
                return Err(StatusCode::FORBIDDEN.into_response());
            }
            Ok(verified)
        }
    }
}
