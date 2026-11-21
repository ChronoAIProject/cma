//! The console's `__Host-` cookies. They are Secure, HttpOnly, host-only and
//! `SameSite=Lax`, which a session needs because it starts after NyxID's
//! cross-site redirect.
use axum::http::{header, HeaderMap, HeaderValue};

pub(crate) const SESSION: &str = "__Host-cmaeg-console";
pub(crate) const LOGIN: &str = "__Host-cmaeg-console-login";

/// The value of cookie `name`; `None` when it is absent, sent twice, or the
/// header is not text, so an injected duplicate never selects a value.
pub(crate) fn get<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        for field in value.to_str().ok()?.split(';') {
            let Some((key, value)) = field.trim().split_once('=') else {
                continue;
            };
            if key == name {
                if found.is_some() {
                    return None;
                }
                found = Some(value);
            }
        }
    }
    found
}

/// `Set-Cookie` for `name`; a browser-session cookie without `max_age`.
pub(crate) fn set(name: &str, value: &str, max_age: Option<u64>) -> Option<HeaderValue> {
    let max_age = max_age.map_or_else(String::new, |seconds| format!("; Max-Age={seconds}"));
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax{max_age}"
    ))
    .ok()
}

pub(crate) fn clear(name: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
    ))
    .expect("cookie names are header-safe")
}
