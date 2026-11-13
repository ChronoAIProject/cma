//! Immutable wire documents; construction and serialization happen once per process.

use std::sync::LazyLock;

use axum::{
    body::Bytes,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::openapi;

static DOCUMENT: LazyLock<CachedDocument> = LazyLock::new(|| {
    #[cfg(test)]
    tests::DOCUMENT_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    CachedDocument::new(&openapi::document())
});
static AGENT_MANAGER_DOCUMENT: LazyLock<CachedDocument> = LazyLock::new(|| {
    #[cfg(test)]
    tests::AGENT_MANAGER_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    CachedDocument::new(&openapi::agent_manager_document())
});

struct CachedDocument {
    bytes: Bytes,
    etag: HeaderValue,
}

impl CachedDocument {
    fn new(document: &impl Serialize) -> Self {
        let bytes = serde_json::to_vec(document).expect("public OpenAPI serializes");
        let etag = format!("\"{:x}\"", Sha256::digest(&bytes))
            .parse()
            .expect("a quoted SHA-256 is a valid ETag");
        Self {
            bytes: bytes.into(),
            etag,
        }
    }

    fn response(&self, headers: &HeaderMap) -> Response {
        // Ambiguous/multiple validators receive a full representation. Keep this
        // deliberately narrow contract shared by the two discovery routes.
        let mut validators = headers.get_all(header::IF_NONE_MATCH).iter();
        let matched = validators
            .next()
            .is_some_and(|value| value == self.etag || value == "*")
            && validators.next().is_none();
        let mut response = if matched {
            StatusCode::NOT_MODIFIED.into_response()
        } else {
            self.bytes.clone().into_response()
        };
        let headers = response.headers_mut();
        if !matched {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        headers.insert(header::ETAG, self.etag.clone());
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        response
    }
}

pub(super) fn document(headers: &HeaderMap) -> Response {
    DOCUMENT.response(headers)
}

pub(super) fn agent_manager_document(headers: &HeaderMap) -> Response {
    AGENT_MANAGER_DOCUMENT.response(headers)
}

/// Initialize both documents on a blocking thread before accepting requests.
/// A panic must fail startup: a poisoned cache cannot safely serve requests.
pub fn warm() {
    LazyLock::force(&DOCUMENT);
    LazyLock::force(&AGENT_MANAGER_DOCUMENT);
}

#[cfg(test)]
mod tests;
