//! Same-origin redirects. Browsers apply CSP `form-action` to every redirect
//! after a form submission, so no POST is ever redirected off-origin: the
//! one off-origin redirect is `GET /console/login`, reached by a click.
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

pub(crate) const HOME: &str = "/console";
pub(crate) const LOGIN: &str = "/console/login";
pub(crate) const CALLBACK: &str = "/console/callback";
pub(crate) const SIGNED_OUT: &str = "/console/signed-out";
pub(crate) const LOGOUT: &str = "/console/logout";
const MAX_RETURN_TO: usize = 512;

/// A same-origin console path to come back to after sign-in, or `/console`.
/// It must start with `/console`, be at most 512 bytes, and contain no `//`,
/// backslash, control character, scheme or authority.
pub(crate) fn return_to(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return HOME.to_owned();
    };
    let console_path = raw == HOME || raw.starts_with("/console/") || raw.starts_with("/console?");
    let safe = console_path
        && raw.len() <= MAX_RETURN_TO
        && !raw.contains("//")
        && !raw.contains('\\')
        && !raw.chars().any(char::is_control)
        && !raw.contains(char::is_whitespace);
    if safe {
        raw.to_owned()
    } else {
        HOME.to_owned()
    }
}

/// The explicit sign-in link for a page at `return_to`.
pub(crate) fn sign_in_href(return_to: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(return_to.as_bytes()).collect();
    format!("{LOGIN}?return_to={encoded}")
}

/// `303 See Other` to a same-origin console path. Anything else is replaced
/// by `/console`, so no caller can produce an off-origin `Location`.
pub(crate) fn see_other(location: &str) -> Response {
    let location = return_to(Some(location));
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).unwrap_or(HeaderValue::from_static(HOME)),
    );
    response
}

/// Where a POST goes once its session is gone: the signed-out page, saying
/// whether it ended by itself.
pub(crate) fn signed_out(expired: bool) -> Response {
    if expired {
        see_other("/console/signed-out?reason=expired")
    } else {
        see_other(SIGNED_OUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_keeps_console_paths_and_replaces_everything_else() {
        for kept in [
            "/console",
            "/console/channels/rec_1?tab=activity&refresh=10",
            "/console?page=2",
        ] {
            assert_eq!(return_to(Some(kept)), kept);
        }
        let long = format!("/console/{}", "a".repeat(504));
        assert_eq!(long.len(), 513);
        for replaced in [
            "",
            "https://evil.example/console",
            "//evil.example/console",
            "/console//evil.example",
            "/console/\\evil",
            "/console/\nSet-Cookie: x",
            "/console/a b",
            "/consolex",
            "/other",
            "javascript:alert(1)",
            "console",
            &long,
        ] {
            assert_eq!(return_to(Some(replaced)), HOME, "{replaced:?}");
        }
        assert_eq!(return_to(None), HOME);
        assert_eq!(return_to(Some(&long[..512])), &long[..512]);
    }

    #[test]
    fn redirects_never_leave_the_origin() {
        for (target, expected) in [
            ("/console/signed-out", "/console/signed-out"),
            ("https://evil.example", HOME),
            ("//evil.example", HOME),
        ] {
            let response = see_other(target);
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], expected);
        }
        assert_eq!(
            signed_out(true).headers()[header::LOCATION],
            "/console/signed-out?reason=expired"
        );
        assert_eq!(
            sign_in_href("/console?page=2&q=a"),
            "/console/login?return_to=%2Fconsole%3Fpage%3D2%26q%3Da"
        );
    }
}
