use axum::http::{header, HeaderMap};

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
