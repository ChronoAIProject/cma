//! Preserve signed wire credentials across authentication that consumes them.
use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};

#[derive(Clone)]
struct WireHeaders(HeaderMap);

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
    let evidence =
        match super::admission::Origin::capture(request.method(), request.uri(), &headers)
            .and_then(|origin| origin.evidence())
        {
            Ok(evidence) => evidence,
            Err(()) => return super::http::unavailable(),
        };
    request.extensions_mut().insert(WireHeaders(headers));
    evidence.scope(next.run(request)).await
}

pub(super) fn headers(request: &Request) -> &HeaderMap {
    request
        .extensions()
        .get::<WireHeaders>()
        .map_or_else(|| request.headers(), |wire| &wire.0)
}
