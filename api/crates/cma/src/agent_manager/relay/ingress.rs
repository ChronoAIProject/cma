//! Preserve signed wire credentials across authentication that consumes them.
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};

#[derive(Clone)]
struct WireHeaders(HeaderMap);

pub(super) async fn file_preview_identity(mut request: Request, next: Next) -> Response {
    if !request.headers().contains_key(super::PROOF) {
        request
            .extensions_mut()
            .insert(crate::auth::relay::ReuseFilePreviewIdentity);
    }
    next.run(request).await
}

pub(super) async fn capture(mut request: Request, next: Next) -> Response {
    // Request-bound read authentication deliberately removes ambient credentials
    // before the handler. A peer must verify the original proof independently;
    // never serialize or trust source-pod authentication extensions.
    let mut headers = HeaderMap::new();
    for (name, value) in request.headers() {
        if super::authentication::forwarded(name.as_str()) {
            headers.append(name, value.clone());
        }
    }
    request.extensions_mut().insert(WireHeaders(headers));
    next.run(request).await
}

pub(super) fn headers(request: &Request) -> &HeaderMap {
    request
        .extensions()
        .get::<WireHeaders>()
        .map_or_else(|| request.headers(), |wire| &wire.0)
}

impl super::Relay {
    pub(crate) async fn classify_audit_hop(
        &self,
        request: Request,
    ) -> Result<(Request, bool), StatusCode> {
        let mut proofs = request.headers().get_all(super::PROOF).iter();
        let Some(proof) = proofs
            .next()
            .filter(|_| proofs.next().is_none())
            .filter(|value| value.len() <= 4096)
            .cloned()
        else {
            return Ok((request, false));
        };
        let body_limit = super::http::body_limit(request.uri().path());
        let capacity = self
            .body_budget
            .try_acquire_many(body_limit as u32)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let (parts, body) = request.into_parts();
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            to_bytes(body, body_limit),
        )
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        let signed = proof.to_str().is_ok_and(|proof| {
            self.authentication
                .verify_signature(proof, &parts.method, &parts.uri, &parts.headers, &bytes)
                .is_ok()
        });
        drop(capacity);
        Ok((Request::from_parts(parts, Body::from(bytes)), signed))
    }
}
