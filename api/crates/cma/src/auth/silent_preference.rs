//! Preference timestamps are bound to durable custody so clock/response order cannot undo logout.
use super::{
    persistence::SecretDigest,
    session_routes::{issued_cookie, request_cookie},
    AuthState, SIGNED_OUT_COOKIE, SIGN_IN_PREFERENCE_LIFETIME,
};
use axum::{
    http::{header, HeaderMap},
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
const SIGNED_IN_COOKIE: &str = "cma_signed_in";

pub(super) fn timestamp(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
pub(super) fn reference(headers: &HeaderMap) -> Option<(SecretDigest, SystemTime)> {
    let (started, binding) = request_cookie(headers, SIGNED_IN_COOKIE)?.split_once('.')?;
    let started = UNIX_EPOCH.checked_add(Duration::from_millis(started.parse().ok()?))?;
    let bytes: [u8; 32] = URL_SAFE_NO_PAD.decode(binding).ok()?.try_into().ok()?;
    Some((SecretDigest::from_bytes(bytes), started))
}
pub(super) async fn suppressed(auth: &AuthState, headers: &HeaderMap) -> bool {
    let Some(signed_out) = request_cookie(headers, SIGNED_OUT_COOKIE) else {
        return false;
    };
    let Ok(signed_out) = signed_out.parse::<u64>() else {
        return true;
    };
    let Some((binding, started)) = reference(headers) else {
        return true;
    };
    if timestamp(started) <= signed_out {
        return true;
    }
    // This reference is preference only, never an authentication credential.
    // The shared binding transaction proves no subsequent explicit logout won.
    !auth
        .store
        .browser_sign_in_preference(&binding, started)
        .await
        .unwrap_or(false)
}
pub(super) fn record_sign_in(
    response: &mut Response,
    binding: &SecretDigest,
    started: SystemTime,
    secure: bool,
) {
    let value = format!(
        "{}.{}",
        timestamp(started),
        URL_SAFE_NO_PAD.encode(binding.as_bytes())
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(
            SIGNED_IN_COOKIE,
            &value,
            secure,
            SIGN_IN_PREFERENCE_LIFETIME.as_secs(),
        ),
    );
}
pub(super) fn same_origin(auth: &AuthState, headers: &HeaderMap) -> bool {
    auth.permits_browser_origin(headers)
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value == "same-origin" || value == "none")
}
pub(super) fn record_sign_out(response: &mut Response, headers: &HeaderMap, auth: &AuthState) {
    if !same_origin(auth, headers) {
        return;
    }
    #[cfg(test)]
    let now = (auth.preference_clock)();
    #[cfg(not(test))]
    let now = SystemTime::now();
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(
            SIGNED_OUT_COOKIE,
            &timestamp(now).to_string(),
            auth.secure_cookies(),
            SIGN_IN_PREFERENCE_LIFETIME.as_secs(),
        ),
    );
}
