//! Public, untrusted browser diagnostics. No identity, content or persistence.
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFault {
    UncaughtError,
    UnhandledRejection,
    ReactError,
    ResourceError,
    ChunkError,
}
impl BrowserFault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UncaughtError => "uncaught_error",
            Self::UnhandledRejection => "unhandled_rejection",
            Self::ReactError => "react_error",
            Self::ResourceError => "resource_error",
            Self::ChunkError => "chunk_error",
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserReport {
    kind: BrowserFault,
}

type Sink = fn(BrowserFault);

pub fn browser_telemetry_router(sink: Sink) -> Router {
    Router::new()
        .route("/api/v1/telemetry/browser", post(report))
        .layer(DefaultBodyLimit::max(256))
        .with_state(sink)
}
#[utoipa::path(post, path = "/api/v1/telemetry/browser", tag = "diagnostics",
    request_body = BrowserReport,
    responses((status = 204, description = "Untrusted diagnostic accepted; no durable receipt"),
    (status = 413, description = "Report exceeds 256 bytes"),
    (status = 422, description = "Unknown category or additional fields"),
    (status = 429, description = "Diagnostic budget exhausted"))
)]
pub(crate) async fn report(
    State(sink): State<Sink>,
    Json(report): Json<BrowserReport>,
) -> StatusCode {
    sink(report.kind);
    StatusCode::NO_CONTENT
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    #[tokio::test]
    async fn accepts_only_bounded_categories_without_private_fields() {
        for (body, status) in [
            (r#"{"kind":"react_error"}"#.to_owned(), 204),
            (r#"{"kind":"private-error-message"}"#.to_owned(), 422),
            (
                r#"{"kind":"react_error","stack":"private"}"#.to_owned(),
                422,
            ),
            (format!("{{\"kind\":\"{}\"}}", "x".repeat(300)), 413),
        ] {
            let response = browser_telemetry_router(|_| {
                CALLS.fetch_add(1, Ordering::Relaxed);
            })
            .oneshot(
                Request::post("/api/v1/telemetry/browser")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(response.status().as_u16(), status);
        }
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(report))]
struct BrowserApi;
pub(super) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    BrowserApi::openapi()
}
