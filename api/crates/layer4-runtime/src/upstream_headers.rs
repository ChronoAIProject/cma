use axum::http::{header, HeaderMap, HeaderValue};

pub(crate) fn encoded(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value.as_bytes() != b"identity")
}

pub(crate) fn headers_contain(headers: &HeaderMap, credential: &[u8]) -> bool {
    headers.iter().any(|(name, value)| {
        crate::upstream_guard::contains(name.as_str().as_bytes(), credential)
            || crate::upstream_guard::contains(value.as_bytes(), credential)
    })
}

pub(crate) fn copy_request_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for (name, value) in source {
        if !owned_header(name.as_str())
            && name.as_str() != super::github_scope::SOURCE_AUTHORITY_HEADER
        {
            target.insert(name, value.clone());
        }
    }
    target.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
}

pub(crate) fn copy_response_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for (name, value) in source {
        if !hop_header(name.as_str()) && name != header::CONTENT_LENGTH {
            target.insert(name, value.clone());
        }
    }
}

fn owned_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "open-sandbox-api-key"
            | "host"
            | "content-length"
            | "accept-encoding"
            | "connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
