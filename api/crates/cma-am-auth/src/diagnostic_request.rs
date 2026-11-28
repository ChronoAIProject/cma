//! Request correlation without raw paths, headers, subjects or payloads.
use axum::{
    body::{Body, HttpBody},
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use layer2_core::diagnostic_context::DiagnosticContext;
use sha2::{Digest, Sha256};
use std::{
    pin::Pin,
    sync::LazyLock,
    task::{Context, Poll},
};

static SALT: LazyLock<[u8; 16]> = LazyLock::new(|| *uuid::Uuid::new_v4().as_bytes());

pub async fn scope(request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str);
    let mut hash = Sha256::new();
    hash.update(*SALT);
    hash.update(
        request
            .headers()
            .get("x-request-id")
            .map_or(&[][..], |value| value.as_bytes()),
    );
    let method = match request.method().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    };
    let context = DiagnosticContext {
        route: format!("{method} {route}").into(),
        request_hash: format!("{:x}", hash.finalize()).into(),
        worker: "request",
        phase: "request",
    };
    let response = context.clone().scope(next.run(request)).await;
    response.map(|body| Body::new(ContextBody { body, context }))
}

pub fn request_span(_request: &Request) -> tracing::Span {
    let context = layer2_core::diagnostic_context::DiagnosticContext::current();
    let (method, path) = context
        .route
        .split_once(' ')
        .unwrap_or(("OTHER", "unmatched"));
    tracing::info_span!(
        parent: None,
        "http.request",
        request_hash = %context.request_hash,
        route = context.route.as_ref(),
        method,
        path,
    )
}

struct ContextBody {
    body: Body,
    context: DiagnosticContext,
}
impl HttpBody for ContextBody {
    type Data = axum::body::Bytes;
    type Error = axum::Error;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        this.context
            .sync_scope(|| Pin::new(&mut this.body).poll_frame(cx))
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}
