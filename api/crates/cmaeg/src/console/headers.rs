//! The security headers every `/console` response carries. The stylesheet is
//! the only resource a page uses, allowed by its hash, so the policy admits
//! no script, image, frame or foreign form target.
use std::sync::LazyLock;

use axum::{
    http::{header, HeaderName, HeaderValue},
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

pub(crate) const STYLE: &str = include_str!("console.css");

static POLICY: LazyLock<HeaderValue> = LazyLock::new(|| {
    let hash = STANDARD.encode(Sha256::digest(STYLE.as_bytes()));
    HeaderValue::from_str(&format!(
        "default-src 'none'; style-src 'sha256-{hash}'; img-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    ))
    .expect("a base64 digest is a valid header value")
});

const PERMISSIONS: &str = "camera=(), microphone=(), geolocation=(), payment=(), usb=()";

/// Add the full header set, replacing anything a handler set.
pub(crate) async fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, POLICY.clone());
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Same-origin keeps the Origin header on form posts and withholds the
    // console URL from NyxID when the sign-in link leaves the site.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS),
    );
    response
}
