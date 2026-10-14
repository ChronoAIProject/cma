//! CSRF protection for console forms. Every POST must come from exactly the
//! public origin and carry `csrf = base64url(HMAC-SHA256(session.csrf_key,
//! action ␟ record_id))`, compared in constant time. The action is the
//! route template, so a token minted for one form is refused by another.
use axum::{
    http::{header, HeaderMap},
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::hmac;
use subtle::ConstantTimeEq;

use super::pages;
use crate::telemetry::names;

/// The ASCII unit separator. Route templates and record ids contain no
/// control characters, so it splits the two parts unambiguously.
const SEPARATOR: char = '\u{1f}';

/// The hidden field value for `action` on `record` (empty for forms that
/// name no record).
pub(crate) fn token(key: &[u8; 32], action: &str, record: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = hmac::sign(&key, format!("{action}{SEPARATOR}{record}").as_bytes());
    URL_SAFE_NO_PAD.encode(tag.as_ref())
}

pub(crate) fn matches(key: &[u8; 32], action: &str, record: &str, presented: &str) -> bool {
    bool::from(
        token(key, action, record)
            .as_bytes()
            .ct_eq(presented.as_bytes()),
    )
}

/// Exactly one `Origin` header, equal to the public origin.
pub(crate) fn origin_matches(headers: &HeaderMap, public_url: &str) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    origins.next().and_then(|value| value.to_str().ok()) == Some(public_url)
        && origins.next().is_none()
}

/// Count a refused POST and answer the generic error page.
pub(crate) fn reject(reason: &'static str) -> Response {
    metrics::counter!(names::CONSOLE_REJECTIONS, names::REASON => reason).increment(1);
    tracing::warn!(reason, "console request refused");
    pages::forbidden()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_bind_the_key_the_action_and_the_record() {
        let key = [7_u8; 32];
        let owner = token(&key, "/console/owner", "");
        assert_eq!(owner.len(), 43);
        assert!(matches(&key, "/console/owner", "", &owner));
        assert!(!matches(&key, "/console/logout", "", &owner));
        assert!(!matches(&key, "/console/owner", "rec_1", &owner));
        assert!(!matches(&[8_u8; 32], "/console/owner", "", &owner));
        assert!(!matches(&key, "/console/owner", "", ""));
        assert!(!matches(&key, "/console/owner", "", &owner[..42]));
    }

    #[test]
    fn the_origin_must_be_exactly_the_public_url_once() {
        let public = "https://gateway.example";
        let with = |values: &[&str]| {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(header::ORIGIN, value.parse().unwrap());
            }
            origin_matches(&headers, public)
        };
        assert!(with(&["https://gateway.example"]));
        for refused in [
            &[][..],
            &["null"],
            &["https://gateway.example/"],
            &["https://gateway.example.evil"],
            &["http://gateway.example"],
            &["https://gateway.example", "https://gateway.example"],
        ] {
            assert!(!with(refused), "{refused:?}");
        }
    }
}
