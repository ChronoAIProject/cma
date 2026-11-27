//! Preserve signed wire credentials across authentication that consumes them.
use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};

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
